use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};

use anyhow::Result;
use clap::Parser;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info, instrument};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(
        short,
        long,
        value_name = "N",
        help = "how many worker processess will be spawned",
        default_value = "4"
    )]
    jobs: Option<u8>,

    #[arg(
        short = 'd',
        long = "dir",
        value_name = "DIR",
        help = "the target directory of generate.py"
    )]
    generated_zip_dir: Option<PathBuf>,

    #[arg(
        short = 'c',
        long = "cmd",
        value_name = "CMD",
        help = "a path or command that can be used to invoke the archipelago launcher"
    )]
    bin: Option<String>,

    #[arg(
        short = 'a',
        long = "args",
        value_name = "ARGS",
        value_parser,
        num_args = 1..,
        // value_delimiter = ' ',
        help = "args passed through to generate.py"
    )]
    options: Vec<String>,
}

fn main() -> Result<()> {
    init_tracing()?;
    let args = Args::parse();
    info!(?args);
    let jobs = args.jobs.unwrap_or(4);
    info!("jobs: {}", jobs);
    let archipelago_dir = args
        .generated_zip_dir
        .unwrap_or_else(|| {
            dirs::home_dir()
                .inspect(|p| info!("defaulting to '{}'", p.display()))
                .unwrap()
                .join("Archipelago")
                .join("output")
        })
        .canonicalize()?;
    info!("checking against '{}' directory", archipelago_dir.display());
    let bin = args
        .bin
        .unwrap_or_else(|| "/run/current-system/sw/bin/archipelago".into());
    info!("archipelago command: '{}'", bin);

    let (ziptx, ziprx) = mpsc::sync_channel::<usize>(4_usize * jobs as usize);

    let initial_zips = how_many_zips(&archipelago_dir).unwrap_or_default();
    info!(
        "there appears to be {} previously generated multiworlds",
        initial_zips
    );

    let _ = thread::spawn(move || {
        info!("generated games checker started");
        let mut max = initial_zips;
        loop {
            if let Ok(msg) = ziprx.recv_timeout(Duration::from_secs(1)) {
                info!(max, msg);
                debug_assert!(max <= msg);
                if msg.gt(&max) {
                    info!(max, msg, "increased");
                    info!("count: {}", msg);
                    break;
                }
                if msg.lt(&max) {
                    info!(
                        "generated games appear to have shrunk, msg: {}, max: {}",
                        msg, max,
                    );
                    max = msg;
                }
            } else {
                debug!("timed out on receiving message from worker threads");
            }
        }
        info!("successfully generated a multiworld, exiting...");
        let _ = signal::kill(Pid::from_raw(0), Signal::SIGINT);
        process::exit(0); // c:
    });

    let count_zips_closure = || how_many_zips(&archipelago_dir);
    debug!("starting workers");
    thread::scope(move |scope| {
        for _ in 1..jobs + 1 {
            let ziptx = ziptx.clone();
            let bin = bin.clone();
            let passthrough_args = args.options.clone();
            scope.spawn(move || loop {
                let Ok(mut child) = generate_multiworld(&bin, passthrough_args.clone()) else {
                    error!("subprocess is malformed");
                    continue;
                };

                debug!("subprocess spawned by worker");
                let Some(mut stdout) = child.stdout.take() else {
                    error!("could not acquire stdout, stopping subprocess");
                    let _ = child.kill();
                    continue;
                };
                debug!("subprocess' stdout acquired by worker");
                let mut output = String::new();
                debug!("reading subprocess stdout");
                let _ = stdout.read_to_string(&mut output);
                debug!(output);
                match count_zips_closure() {
                    Ok(counted_zips) => {
                        let _ = {
                            info!(counted_zips);
                            debug!("{}", output);
                            ziptx.send(counted_zips)
                        };
                    }
                    Err(e) => {
                        error!(?e);
                        let _ = child.kill();
                        error!(output)
                    }
                };
            });
        }
        debug!("workers spawned");
    });
    info!("workers joined");
    Ok(())
}

#[instrument]
fn how_many_zips(folder: &Path) -> Result<usize> {
    let dir = fs::read_dir(folder)?;
    let count = dir
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext == std::ffi::OsStr::new("zip"))
        })
        .count();
    Ok(count)
}

#[instrument]
fn generate_multiworld(bin: &str, args: Vec<String>) -> Result<Child> {
    debug!("spawning generator");
    let generator = if !args.is_empty() {
        let args = Vec::from_iter(args.into_iter().map(|a| format!("--{}", a)));
        info!("calling `{bin} \"Generate\"` with args {:?}", args);
        Command::new(bin)
            .arg("Generate")
            .args(args)
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
    } else {
        Command::new(bin)
            .arg("Generate")
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()
    };

    debug!(?generator);
    let mut child = generator?;
    match child.stdin.take() {
        Some(mut stdin) => {
            let _ = stdin.write_all(b"\n");
        }
        None => {
            error!("unable to access subprocess' stdin");
        }
    }
    Ok(child)
}

fn init_tracing() -> Result<()> {
    let _ = dotenvy::dotenv();
    let subscriber = tracing_subscriber::fmt().pretty().finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    info!("init tracing and logging");

    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    info!("env filter: {}", filter);

    let console_layer = fmt::Layer::default()
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .with_writer(std::io::stdout);

    let registry = Registry::default();

    #[cfg(unix)]
    let registry = {
        use tracing_journald;
        let journald_layer = tracing_journald::layer()?;
        registry.with(journald_layer)
    };

    registry.with(filter).with(console_layer).try_init()?;
    Ok(())
}
