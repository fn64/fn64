//! Build-time selection for the OoT typed-Rust execution lane.
//!
//! Kept free of Cargo/build-script state so the loud selection contract can
//! be tested without any private ROM or generated game output.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // host binary includes this file only for its digest parser
pub enum RsExecution {
    Function,
    Block { artifact: PathBuf },
}

#[allow(dead_code)] // build script owns selection; host binary owns digest parsing
pub fn select_rs_execution(
    mode: Option<OsString>,
    artifact: Option<OsString>,
) -> Result<RsExecution, String> {
    let mode = match mode {
        None => "function".to_owned(),
        Some(value) => value.into_string().map_err(|_| {
            "FN64_RS_EXECUTION must be valid Unicode (`function` or `block`)".to_owned()
        })?,
    };
    match mode.as_str() {
        "function" => {
            if artifact.is_some() {
                return Err(
                    "RECOMP_RS_BLOCK_PROGRAM is set but FN64_RS_EXECUTION is not `block`; refusing to ignore an explicitly supplied pack artifact"
                        .to_owned(),
                );
            }
            Ok(RsExecution::Function)
        }
        "block" => {
            let artifact = artifact.ok_or_else(|| {
                "FN64_RS_EXECUTION=block requires RECOMP_RS_BLOCK_PROGRAM to name the generated pack Rust source"
                    .to_owned()
            })?;
            if artifact.is_empty() {
                return Err("RECOMP_RS_BLOCK_PROGRAM must be a nonempty filesystem path".to_owned());
            }
            let artifact = PathBuf::from(artifact);
            if !artifact.is_file() {
                return Err(format!(
                    "RECOMP_RS_BLOCK_PROGRAM={} is not a regular file",
                    artifact.display()
                ));
            }
            Ok(RsExecution::Block { artifact })
        }
        other => Err(format!(
            "FN64_RS_EXECUTION must be `function` or `block`, got {other:?}"
        )),
    }
}

#[allow(dead_code)] // used by the host binary; the build-script copy only selects paths
pub fn parse_lowercase_sha256(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err(format!(
            "artifact SHA-256 must contain 64 lowercase hexadecimal digits, got {}",
            value.len()
        ));
    }
    let mut bytes = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hexadecimal_nibble(pair[0])
            .ok_or_else(|| format!("artifact SHA-256 has invalid digit at byte {}", index * 2))?;
        let low = hexadecimal_nibble(pair[1]).ok_or_else(|| {
            format!(
                "artifact SHA-256 has invalid digit at byte {}",
                index * 2 + 1
            )
        })?;
        bytes[index] = (high << 4) | low;
    }
    Ok(bytes)
}

#[allow(dead_code)] // see parse_lowercase_sha256
fn hexadecimal_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_artifact() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fn64-oot-block-program-config-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"synthetic pack contract fixture").unwrap();
        path
    }

    #[test]
    fn function_lane_is_the_only_implicit_selection() {
        assert_eq!(select_rs_execution(None, None), Ok(RsExecution::Function));
        assert!(select_rs_execution(None, Some("pack.rs".into()))
            .unwrap_err()
            .contains("refusing to ignore"));
    }

    #[test]
    fn block_lane_requires_an_existing_explicit_artifact() {
        assert!(select_rs_execution(Some("block".into()), None)
            .unwrap_err()
            .contains("requires RECOMP_RS_BLOCK_PROGRAM"));
        assert!(
            select_rs_execution(Some("block".into()), Some("/missing/fn64-pack.rs".into()))
                .unwrap_err()
                .contains("is not a regular file")
        );
        let artifact = temporary_artifact();
        assert_eq!(
            select_rs_execution(
                Some("block".into()),
                Some(artifact.clone().into_os_string())
            ),
            Ok(RsExecution::Block {
                artifact: artifact.clone()
            })
        );
        std::fs::remove_file(artifact).unwrap();
    }

    #[test]
    fn unknown_mode_is_rejected() {
        assert!(select_rs_execution(Some("auto".into()), None)
            .unwrap_err()
            .contains("must be `function` or `block`"));
    }

    #[test]
    fn artifact_digest_parser_is_exact_and_lowercase() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let parsed = parse_lowercase_sha256(digest).unwrap();
        assert_eq!(
            parsed[..8],
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]
        );
        assert!(parse_lowercase_sha256("00")
            .unwrap_err()
            .contains("64 lowercase"));
        assert!(parse_lowercase_sha256(
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef"
        )
        .unwrap_err()
        .contains("invalid digit"));
    }
}
