//! `flick` opens the overlay; subcommands run headless.
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use flick::{
    capture::{CaptureBackend, CaptureRequest, wlr::WlrBackend},
    config::Launch,
    geometry::Rect,
    output::{clipboard, save},
    record::{Audio, Recorder},
};
use std::{
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Use the alternative behavior after selecting
    #[arg(long)]
    alt: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the settings window
    Settings,
    /// List outputs
    Outputs,
    /// Capture a PNG to a file, or to the clipboard without --file
    Shot {
        #[arg(long, value_parser = parse_rect, allow_hyphen_values = true)]
        region: Option<Rect>,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Record MP4 for a fixed time
    Record {
        #[arg(long, value_parser = parse_rect, allow_hyphen_values = true)]
        region: Option<Rect>,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value_t = 5)]
        seconds: u64,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// Without system audio
        #[arg(long)]
        no_audio: bool,
        /// With the microphone
        #[arg(long)]
        mic: bool,
    },
}

fn parse_rect(s: &str) -> Result<Rect> {
    let v = s
        .split(',')
        .map(|n| n.trim().parse::<i32>())
        .collect::<Result<Vec<_>, _>>()?;
    match v[..] {
        [x, y, w, h] if w > 0 && h > 0 => Ok(Rect::new(x, y, w, h)),
        _ => bail!("expected x,y,width,height"),
    }
}

fn request(
    backend: &mut WlrBackend,
    output: Option<String>,
    region: Option<Rect>,
    cursor: bool,
) -> Result<CaptureRequest> {
    let output = match output {
        Some(o) => o,
        None => {
            let outputs = backend.outputs()?;
            outputs.into_iter().next().context("no outputs")?.name
        }
    };
    Ok(CaptureRequest {
        output,
        region,
        cursor,
        interval: Duration::ZERO,
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let launch = match (&cli.command, cli.alt) {
        (None, false) => Some(Launch::Main),
        (None, true) => Some(Launch::Alternative),
        (Some(Cmd::Settings), _) => Some(Launch::Settings),
        _ => None,
    };
    let (Some(command), None) = (cli.command, launch) else {
        let launch = launch.expect("one of the two");
        return match flick::app::run(launch).get() {
            0 => ExitCode::SUCCESS,
            _ => ExitCode::FAILURE,
        };
    };
    match headless(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flick: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn headless(command: Cmd) -> Result<()> {
    let mut backend = WlrBackend::connect()?;
    match command {
        Cmd::Settings => unreachable!("run by the app"),
        Cmd::Outputs => {
            for o in backend.outputs()? {
                let (w, h) = o.logical;
                println!(
                    "{} {}x{} scale {} logical {w}x{h}",
                    o.name, o.width, o.height, o.scale
                );
            }
        }
        Cmd::Shot {
            region,
            output,
            file,
        } => {
            let req = request(&mut backend, output, region, false)?;
            let png = save::encode_png(&backend.frame(&req)?)?;
            match file {
                Some(f) => std::fs::write(f, png)?,
                None => clipboard::copy_png(&png)?,
            }
        }
        Cmd::Record {
            region,
            output,
            file,
            seconds,
            fps,
            no_audio,
            mic,
        } => {
            let req = request(&mut backend, output, region, true)?;
            let mut rec = Recorder::start(
                Box::new(backend),
                req,
                fps,
                Audio {
                    system: !no_audio,
                    microphone: mic,
                },
                file,
            )?;
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end && !rec.has_failed() {
                std::thread::sleep(Duration::from_millis(50));
            }
            let s = rec.stats();
            let unique = s.written - s.repeated;
            eprintln!(
                "captured {} ({:.1}/s), written {}, new {} ({:.1}%), worst lag {} frames",
                s.captured,
                s.captured as f64 / seconds as f64,
                s.written,
                unique,
                100.0 * unique as f64 / s.written.max(1) as f64,
                s.worst_lag,
            );
            println!("{}", rec.stop()?.display());
        }
    }
    Ok(())
}
