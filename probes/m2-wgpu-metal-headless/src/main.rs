use std::process::ExitCode;

use fn64_m2_wgpu_metal_headless::Cli;

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("metal_caps: {error}");
            return ExitCode::from(2);
        }
    };
    let output = fn64_m2_wgpu_metal_headless::run(&cli);
    println!("{}", output.json);
    ExitCode::from(output.exit_code as u8)
}
