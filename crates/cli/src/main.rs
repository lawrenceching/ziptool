use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use zip_tool_core::{Options, compress_dir, extract, parse_size};

#[derive(Parser)]
#[command(
    name = "zip-tool",
    version,
    about = "Batch-compress directories before moving a large data disk"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 批量压缩目录(每个目录一个压缩包)
    Compress {
        /// 包含待压缩目录的根路径
        #[arg(short, long)]
        input: PathBuf,
        /// 压缩包输出目录
        #[arg(short, long)]
        output: PathBuf,
        /// 分包大小,如 2g / 500m;缺省不分包
        #[arg(short, long)]
        split: Option<String>,
        /// 压缩密码
        #[arg(short, long)]
        password: Option<String>,
        /// 使用随机包名
        #[arg(short, long)]
        random_name: bool,
    },
    /// 解压/合并分包
    Extract {
        /// 压缩包或第一个分卷(.zip / .zip.001)
        #[arg(short, long)]
        input: PathBuf,
        /// 解压目标目录
        #[arg(short, long)]
        output: PathBuf,
        /// 解压密码
        #[arg(short, long)]
        password: Option<String>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Compress {
            input,
            output,
            split,
            password,
            random_name,
        } => {
            let split_size = match split.as_deref().map(parse_size) {
                Some(Ok(v)) => Some(v),
                Some(Err(e)) => {
                    eprintln!("error: invalid --split {split:?}: {e}");
                    return ExitCode::from(2);
                }
                None => None,
            };
            let opts = Options {
                split_size,
                password,
                random_name,
            };
            match compress_dir(&input, &output, &opts, &|p| {
                eprint!("\r[{}/{}] {}        ", p.done, p.total, p.entry);
                ControlFlow::Continue(())
            }) {
                Ok(report) => {
                    eprintln!();
                    for e in &report.entries {
                        println!("{} -> {}", e.original, e.archive.display());
                    }
                    if report.cancelled {
                        eprintln!("cancelled, {} of entries done", report.entries.len());
                        return ExitCode::from(130);
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Extract {
            input,
            output,
            password,
        } => match extract(&input, &output, password.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
