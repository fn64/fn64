use fn64_boot_harness::{
    materialize_release_program_build_receipt, NativeArchiveReceiptInput,
    ReleaseProgramBuildReceiptInput,
};
use std::{env, ffi::OsString, path::PathBuf, process};

fn main() {
    match parse_arguments(env::args_os().skip(1)) {
        Ok(ParsedCommand::Help) => println!("{}", usage()),
        Ok(ParsedCommand::Materialize {
            output,
            child,
            input,
        }) => match materialize_release_program_build_receipt(output, child, input) {
            Ok(receipt) => {
                println!("receipt_sha256={}", receipt.receipt_sha256);
                println!("execution_source={:?}", receipt.execution_source);
                eprintln!("identity co-binding only: this receipt is not compile/link attestation");
            }
            Err(error) => {
                eprintln!("materialize-release-program-build-receipt: {error}");
                process::exit(1);
            }
        },
        Err(error) => {
            eprintln!(
                "materialize-release-program-build-receipt: {error}\n\n{}",
                usage()
            );
            process::exit(2);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedCommand {
    Help,
    Materialize {
        output: PathBuf,
        child: PathBuf,
        input: ReleaseProgramBuildReceiptInput,
    },
}

#[derive(Default)]
struct Options {
    output: Option<PathBuf>,
    child: Option<PathBuf>,
    archives: Vec<NativeArchiveReceiptInput>,
    identity_wire: Option<PathBuf>,
    pack: Option<PathBuf>,
    expected_program_sha256: Option<String>,
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<ParsedCommand, String> {
    let mut arguments = arguments.into_iter();
    let mode = arguments
        .next()
        .ok_or_else(|| "missing receipt lane".to_owned())?;
    if mode == "--help" || mode == "-h" {
        if arguments.next().is_some() {
            return Err("--help takes no other arguments".to_owned());
        }
        return Ok(ParsedCommand::Help);
    }
    let mode = mode
        .into_string()
        .map_err(|_| "receipt lane must be valid Unicode".to_owned())?;
    if !matches!(
        mode.as_str(),
        "native-archives" | "typed-observed-function" | "typed-block"
    ) {
        return Err(format!("unknown receipt lane {mode:?}"));
    }

    let mut options = Options::default();
    while let Some(flag) = arguments.next() {
        let flag = flag
            .into_string()
            .map_err(|_| "option name must be valid Unicode".to_owned())?;
        match flag.as_str() {
            "--output" => set_once(
                &mut options.output,
                PathBuf::from(next_value(&mut arguments, "--output")?),
                "--output",
            )?,
            "--child" => set_once(
                &mut options.child,
                PathBuf::from(next_value(&mut arguments, "--child")?),
                "--child",
            )?,
            "--archive" => {
                let label = next_value(&mut arguments, "--archive LABEL")?
                    .into_string()
                    .map_err(|_| "--archive label must be valid Unicode".to_owned())?;
                let path = PathBuf::from(next_value(&mut arguments, "--archive LABEL PATH")?);
                options
                    .archives
                    .push(NativeArchiveReceiptInput { label, path });
            }
            "--identity-wire" => set_once(
                &mut options.identity_wire,
                PathBuf::from(next_value(&mut arguments, "--identity-wire")?),
                "--identity-wire",
            )?,
            "--pack" => set_once(
                &mut options.pack,
                PathBuf::from(next_value(&mut arguments, "--pack")?),
                "--pack",
            )?,
            "--expected-program-sha256" => {
                let value = next_value(&mut arguments, "--expected-program-sha256")?
                    .into_string()
                    .map_err(|_| "--expected-program-sha256 must be valid Unicode".to_owned())?;
                set_once(
                    &mut options.expected_program_sha256,
                    value,
                    "--expected-program-sha256",
                )?;
            }
            _ => return Err(format!("unknown option {flag:?}")),
        }
    }

    let output = options
        .output
        .ok_or_else(|| "missing --output".to_owned())?;
    let child = options.child.ok_or_else(|| "missing --child".to_owned())?;
    let input = match mode.as_str() {
        "native-archives" => {
            if options.archives.is_empty() {
                return Err("native-archives requires at least one --archive LABEL PATH".to_owned());
            }
            reject_present(&options.identity_wire, "--identity-wire", &mode)?;
            reject_present(&options.pack, "--pack", &mode)?;
            reject_present(
                &options.expected_program_sha256,
                "--expected-program-sha256",
                &mode,
            )?;
            ReleaseProgramBuildReceiptInput::NativeArchives {
                archives: options.archives,
            }
        }
        "typed-observed-function" => {
            reject_nonempty(&options.archives, "--archive", &mode)?;
            reject_present(&options.pack, "--pack", &mode)?;
            reject_present(
                &options.expected_program_sha256,
                "--expected-program-sha256",
                &mode,
            )?;
            ReleaseProgramBuildReceiptInput::TypedObservedFunction {
                identity_wire: options
                    .identity_wire
                    .ok_or_else(|| "typed-observed-function requires --identity-wire".to_owned())?,
            }
        }
        "typed-block" => {
            reject_nonempty(&options.archives, "--archive", &mode)?;
            reject_present(&options.identity_wire, "--identity-wire", &mode)?;
            ReleaseProgramBuildReceiptInput::TypedBlock {
                pack: options
                    .pack
                    .ok_or_else(|| "typed-block requires --pack".to_owned())?,
                expected_program_sha256: options
                    .expected_program_sha256
                    .ok_or_else(|| "typed-block requires --expected-program-sha256".to_owned())?,
            }
        }
        _ => unreachable!("receipt lane was validated"),
    };
    Ok(ParsedCommand::Materialize {
        output,
        child,
        input,
    })
}

fn next_value(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<OsString, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{option} may be supplied only once"));
    }
    Ok(())
}

fn reject_present<T>(value: &Option<T>, option: &str, mode: &str) -> Result<(), String> {
    if value.is_some() {
        Err(format!("{option} is not valid for {mode}"))
    } else {
        Ok(())
    }
}

fn reject_nonempty<T>(value: &[T], option: &str, mode: &str) -> Result<(), String> {
    if value.is_empty() {
        Ok(())
    } else {
        Err(format!("{option} is not valid for {mode}"))
    }
}

fn usage() -> &'static str {
    "usage:\n  materialize-release-program-build-receipt native-archives --output ABSOLUTE.json --child ABSOLUTE_EXECUTABLE --archive LABEL ABSOLUTE_ARCHIVE [--archive LABEL ABSOLUTE_ARCHIVE ...]\n  materialize-release-program-build-receipt typed-observed-function --output ABSOLUTE.json --child ABSOLUTE_EXECUTABLE --identity-wire ABSOLUTE_WIRE\n  materialize-release-program-build-receipt typed-block --output ABSOLUTE.json --child ABSOLUTE_EXECUTABLE --pack ABSOLUTE_PACK --expected-program-sha256 LOWERCASE_SHA256\n\nThe receipt co-binds exact identities; it is not compile/link attestation."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_all_three_receipt_lanes() {
        let native = parse_arguments(strings(&[
            "native-archives",
            "--child",
            "/private/child",
            "--archive",
            "section-bridge",
            "/private/bridge.a",
            "--output",
            "/private/receipt.json",
            "--archive",
            "generated-code",
            "/private/generated.a",
        ]))
        .unwrap();
        assert!(matches!(
            native,
            ParsedCommand::Materialize {
                input: ReleaseProgramBuildReceiptInput::NativeArchives { archives },
                ..
            } if archives.len() == 2
        ));

        let observed = parse_arguments(strings(&[
            "typed-observed-function",
            "--output",
            "/private/receipt.json",
            "--child",
            "/private/child",
            "--identity-wire",
            "/private/source.wire",
        ]))
        .unwrap();
        assert!(matches!(
            observed,
            ParsedCommand::Materialize {
                input: ReleaseProgramBuildReceiptInput::TypedObservedFunction { .. },
                ..
            }
        ));

        let block = parse_arguments(strings(&[
            "typed-block",
            "--output",
            "/private/receipt.json",
            "--child",
            "/private/child",
            "--pack",
            "/private/program.pack",
            "--expected-program-sha256",
            &"11".repeat(32),
        ]))
        .unwrap();
        assert!(matches!(
            block,
            ParsedCommand::Materialize {
                input: ReleaseProgramBuildReceiptInput::TypedBlock { .. },
                ..
            }
        ));
    }

    #[test]
    fn rejects_cross_lane_and_repeated_options() {
        let cross_lane = parse_arguments(strings(&[
            "typed-observed-function",
            "--output",
            "/private/receipt.json",
            "--child",
            "/private/child",
            "--identity-wire",
            "/private/source.wire",
            "--pack",
            "/private/program.pack",
        ]))
        .unwrap_err();
        assert!(cross_lane.contains("--pack is not valid"));

        let repeated = parse_arguments(strings(&[
            "typed-block",
            "--output",
            "/private/one.json",
            "--output",
            "/private/two.json",
            "--child",
            "/private/child",
            "--pack",
            "/private/program.pack",
            "--expected-program-sha256",
            &"11".repeat(32),
        ]))
        .unwrap_err();
        assert!(repeated.contains("--output may be supplied only once"));
    }
}
