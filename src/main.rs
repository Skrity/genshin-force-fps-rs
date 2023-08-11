use core::time::Duration;
use std::error::Error;
use std::thread::sleep;

use genshin_force_fps::logger::TinyLogger;
use genshin_force_fps::process::module::Module;
use genshin_force_fps::process::Process;

use log::{error, info};
use windows::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};

const HELP: &str = "\
Genshin Force FPS

USAGE:
  genshin-force-fps.exe [OPTIONS] -- [GAME_ARGS]
OPTIONS:
  -h, --help                Prints help information
  -n, --no-disable-vsync    Don't forcibly disable VSync
  -f, --fps NUMBER          Force game FPS, defaults to 120
  -c, --cwd PATH            Path to working dir that game process runs on
  -o, --open PATH           Path to GenshinImpact.exe/YuanShen.exe, can be
                            omitted if it's installed on default location (C:)
ARGS:
  [GAME_ARGS]               Unity player arguments passing to game executable,
                            https://docs.unity3d.com/Manual/PlayerCommandLineArguments.html
EXAMPLE:
  # Force FPS to 120 and specify game path
  genshin-force-fps.exe -f 120 -o C:\\path\\to\\GenshinImpact.exe
  # Force FPS to 144 and append Unity cli arguments, assuming the game was
  # installed on default location
  genshin-force-fps.exe -f 144 -- -screen-width 1600 -screen-height 900 -screen-fullscreen 0
";

const DEFAULT_GAME_PATHS: &[&'static str] = &[
    "C:\\Program Files\\Genshin Impact\\Genshin Impact Game\\GenshinImpact.exe",
    "C:\\Program Files\\Genshin Impact\\Genshin Impact Game\\YuanShen.exe",
];

struct Args {
    game_path: Option<String>,
    game_cwd: Option<String>,
    fps: i32,
    disable_vsync: bool,
    game_args: Vec<String>,
}

fn parse_env_args() -> Result<Args, lexopt::Error> {
    use lexopt::prelude::*;

    let mut game_path: Option<String> = None;
    let mut game_cwd: Option<String> = None;
    let mut fps: i32 = 120;
    let mut disable_vsync: bool = true;
    let mut game_args: Vec<String> = vec![];

    let mut parser = lexopt::Parser::from_env();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                println!("{}", HELP);
                std::process::exit(0);
            }
            Short('n') | Long("no-disable-vsync") => {
                disable_vsync = false;
            }
            Short('f') | Long("fps") => {
                fps = parser.value()?.parse()?;
                fps = ::core::cmp::max(1, fps);
            }
            Short('c') | Long("cwd") => {
                game_cwd = Some(parser.value()?.parse()?);
            }
            Short('o') | Long("open") => {
                game_path = Some(parser.value()?.parse()?);
            }
            Value(val) => {
                game_args.push(val.into_string()?);
            }
            _ => return Err(arg.unexpected()),
        }
    }

    Ok(Args {
        game_path,
        game_cwd,
        fps,
        disable_vsync,
        game_args,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    TinyLogger.init()?;
    let Args {
        game_path,
        game_cwd,
        mut game_args,
        fps,
        disable_vsync,
    } = parse_env_args()?;

    let game_path = match game_path {
        Some(s) => {
            if !path_exists(&s) {
                eprintln!("{}", HELP);
                eprintln!("Game path {} doesn't exists!", s);
                std::process::exit(1);
            }
            s
        }
        None => loop {
            if let Some(possible_path) = game_args.first() {
                if path_exists(possible_path) {
                    break game_args.remove(0);
                }
            }
            let res = DEFAULT_GAME_PATHS.iter().find(|i| path_exists(i));
            if let Some(&s) = res {
                break s.to_owned();
            } else {
                eprintln!("{}", HELP);
                eprintln!("Please specify the game path with option -o");
                std::process::exit(1);
            }
        },
    };
    let game_args = game_args.join(" ");
    if game_args.len() > 0 {
        info!("launching {} {}", game_path, game_args);
    } else {
        info!("launching {}", game_path);
    }
    let ps = Process::create(&game_path, game_cwd.as_deref(), &game_args)?;
    let m_up = loop {
        sleep(Duration::from_millis(200));
        match ps.get_module("UnityPlayer.dll") {
            Ok(m) => break m,
            Err(s) => {
                error!("{}", s);
            }
        }
        if !ps.is_active() {
            return Ok(());
        }
    };

    sleep(Duration::from_millis(5000));
    let m_ua = loop {
        sleep(Duration::from_millis(200));
        match ps.get_module("UserAssembly.dll") {
            Ok(m) => break m,
            Err(s) => {
                error!("{}", s);
            }
        }
        if !ps.is_active() {
            return Ok(());
        }
    };

    let p_fps = scan_fps_ptr(&ps, &m_up, &m_ua)?;
    let p_vsync = scan_vsync_ptr(&ps, &m_up)?;

    info!("scan success: p_fps:{:?}, p_vsync:{:?}", p_fps, p_vsync);
    drop(m_up);

    loop {
        if !ps.is_active() {
            return Ok(());
        }
        sleep(Duration::from_secs(1));

        let res = ps.read::<i32>(p_fps);
        if let Ok(v) = res {
            if v != fps && v >= 0 {
                let res = ps.write::<i32>(p_fps, &fps);
                if res.is_err() {
                    error!("failed to write FPS");
                } else {
                    info!("force FPS: {} -> {}", v, fps);
                }
            }
        }

        if disable_vsync {
            let res = ps.read::<i32>(p_vsync);
            if let Ok(v) = res {
                if v != 0 {
                    let res = ps.write::<i32>(p_vsync, &0);
                    if res.is_err() {
                        error!("failed to write VSync");
                    } else {
                        info!("VSync forcibly disabled");
                    }
                }
            }
        }
    }
}

#[inline]
unsafe fn extract_address(
    m: &Module,
    p_inst: *mut u8,
    address_offset: usize,
    inst_len: usize,
) -> *mut u8 {
    let rel = (m.snapshot_addr(p_inst.add(address_offset)) as *mut i32).read_unaligned() as isize;
    p_inst.offset(rel + inst_len as isize)
}

fn scan_fps_ptr(ps: &Process, m_up: &Module, m_ua: &Module) -> Result<*mut u8, Box<dyn Error>> {
    let p_fps_anchor = m_ua
        .pattern_scan("E8 ? ? ? ? 85 C0 7E 07 E8 ? ? ? ? EB 05")
        .ok_or("FPS anchor pattern not found, try updating this tools")?;
    unsafe {
        let p_func_indirect = extract_address(m_ua, p_fps_anchor, 1, 5);

        let pp_func_fps = extract_address(m_ua, p_func_indirect, 3, 7);

        let mut p_func_fps = loop {
            let p = ps.read::<u64>(pp_func_fps)?;
            if p == 0 {
                sleep(Duration::from_millis(200));
                continue;
            }
            break (p as *mut u8);
        };

        loop {
            let inst = *m_up.snapshot_addr(p_func_fps);
            match inst {
                // CALL
                0xe8 | 0xe9 => {
                    p_func_fps = extract_address(m_up, p_func_fps, 1, 5);
                    continue;
                }
                _ => break,
            }
        }

        let p_fps = extract_address(m_up, p_func_fps, 2, 6);
        Ok(p_fps)
    }
}

/**
 * 3.6 or before
 */
fn _scan_fps_ptr_legacy(m_up: &Module) -> Result<*mut u8, Box<dyn Error>> {
    let p_fps_anchor = m_up
        .pattern_scan("7F 0F 8B 05 ? ? ? ?")
        .ok_or("FPS anchor pattern not found")?;
    unsafe {
        let p_fps = extract_address(m_up, p_fps_anchor, 4, 8);
        Ok(p_fps)
    }
}

fn scan_vsync_ptr(ps: &Process, m_up: &Module) -> Result<*mut u8, Box<dyn Error>> {
    let p_vsync_anchor = m_up
        .pattern_scan("E8 ? ? ? ? 8B E8 49 8B 1E")
        .ok_or("VSync anchor pattern not found, try updating this tools")?;
    unsafe {
        let p_func_read_vsync = extract_address(m_up, p_vsync_anchor, 1, 5);

        let pp_vsync_base = extract_address(m_up, p_func_read_vsync, 3, 7);

        let vsync_offset =
            (m_up.snapshot_addr(p_func_read_vsync.add(9)) as *mut i32).read_unaligned() as isize;

        let p_vsync_base = loop {
            let p = ps.read::<u64>(pp_vsync_base)?;
            if p == 0 {
                sleep(Duration::from_millis(200));
                continue;
            }
            break (p as *mut u8);
        };
        Ok(p_vsync_base.offset(vsync_offset))
    }
}

fn path_exists(path: &str) -> bool {
    unsafe {
        let attrs = GetFileAttributesW(path);
        attrs != INVALID_FILE_ATTRIBUTES
    }
}
