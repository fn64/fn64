#![forbid(unsafe_code)]

use std::io::{self, Read};

use serde::{de::DeserializeOwned, Serialize};

#[path = "../wire.rs"]
#[allow(dead_code)] // Shared binary-private protocol; this binary owns evaluation, not runner issuance.
mod wire;

use wire::{evaluate, inspect, EvaluationRequest, ReplayFixture};

fn stdin_json<T: DeserializeOwned>() -> Result<T, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn stdout_json<T: Serialize>(value: &T) -> Result<(), Box<dyn std::error::Error>> {
    serde_json::to_writer(io::stdout().lock(), value)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("inspect") => stdout_json(&inspect(&stdin_json::<ReplayFixture>()?)?)?,
        Some("evaluate") => stdout_json(&evaluate(&stdin_json::<EvaluationRequest>()?)?)?,
        _ => return Err("usage: fn64-render-conformance-verifier inspect|evaluate".into()),
    }
    Ok(())
}
