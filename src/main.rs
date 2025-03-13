use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use std::{fs, thread};

use anyhow::Result;
use clap::Parser;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info, instrument};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    jobs: Option<u8>,

    #[arg(short, long)]
    generated_zip_dir: Option<PathBuf>,

    #[arg(short, long)]
    bin: Option<String>,
}

fn main() -> Result<()> {
    init_tracing()?;
    let args = Args::parse();
    info!(?args);
    let jobs = args.jobs.unwrap_or(32);
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

    let (tx, rx) = mpsc::sync_channel::<usize>(4_usize * jobs as usize);

    let cancel = Arc::new(AtomicBool::new(false));
    let checker_clone = cancel.clone();

    let initial_zips = how_many_zips(&archipelago_dir).unwrap_or_default();
    info!(
        "there appears to be {} previously generated multiworlds",
        initial_zips
    );

    let checker_thread = Arc::new(thread::spawn(move || {
        info!("generated games checker started");
        let cancel = checker_clone;
        let mut max = initial_zips;
        loop {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            if let Ok(msg) = rx.recv_timeout(Duration::from_secs(1)) {
                info!(max, msg);
                debug_assert!(max < msg);
                if msg.gt(&max) {
                    info!(max, msg, "icreased");
                    info!("count: {}", msg);
                    cancel.store(true, Ordering::Release);
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
            debug!("parking");
            thread::park_timeout(Duration::from_secs(5));
        }
        info!("successfully generated a multiworld, forcefully exiting...");
        process::exit(0);
    }));

    let count_zips_closure = || how_many_zips(&archipelago_dir);
    debug!("starting workers");
    thread::scope(move |scope| {
        for _ in 1..jobs + 1 {
            let tx = tx.clone();
            let cancel = cancel.clone();
            let unpark_me = checker_thread.clone();
            let bin = bin.clone();
            scope.spawn(move || loop {
                let Ok(mut child) = generate_multiworld(&bin) else {
                    error!("subprocess is malformed");
                    continue;
                };
                let cancelled = cancel.load(Ordering::Acquire);
                if cancelled {
                    info!("stopping subprocess");
                    let _ = child.kill();
                    break;
                }
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
                debug!("stdout\n\n{}\n\n", output);
                debug!("awaiting generator result");
                let _exit = child.wait();
                match count_zips_closure() {
                    Ok(c) => {
                        let _ = {
                            let thread = unpark_me.thread();
                            // debug!("unparking: {:?}", thread.id());
                            thread.unpark();
                            info!(c);
                            info!("{}", output);
                            tx.send(c)
                        };
                    }
                    Err(e) => {
                        error!(?e);
                        error!(output)
                    }
                };
            });
        }
        debug!("workers spawned");
    });
    debug!("workers joined");
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
fn generate_multiworld(bin: &str) -> Result<Child> {
    debug!("spawning generator");
    let generator = Command::new(bin)
        .arg("Generate")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn();

    debug!(?generator);
    let mut child = generator?;
    match child.stdin.take() {
        Some(mut stdin) => {
            // archipelago's Generate.py script is annoying and swallows errors and requires a newline to continue
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
    // temporarily set a subscriber during initialisation
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
