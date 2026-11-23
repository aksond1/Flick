//! `flick` opens the overlay; subcommands run headless.
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use flick::{
    capture::{CaptureBackend, CaptureRequest, wlr::WlrBackend},
    geometry::Rect,
    output::{clipboard, save},
    record::Recorder,
};
use std::{
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
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
        #[arg(long)]
        no_audio: bool,
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
    let Some(command) = Cli::parse().command else {
        return match flick::app::run().get() {
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
        Cmd::Outputs => {
            for o in backend.outputs()? {
                println!("{} {}x{} scale {}", o.name, o.width, o.height, o.scale);
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
        } => {
            let req = request(&mut backend, output, region, true)?;
            let mut rec = Recorder::start(Box::new(backend), req, fps, !no_audio, file)?;
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end && !rec.has_failed() {
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("{}", rec.stop()?.display());
        }
    }
    Ok(())
}
