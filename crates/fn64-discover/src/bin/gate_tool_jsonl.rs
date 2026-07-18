//! Conformance-only validator for provider JSONL fixtures.
//!
//! This derives the expectation from the stream header, so it proves schema,
//! completion, range, and digest consistency only. Production ingestion must
//! supply an independently constructed [`ToolAdapterExpectation`].

use fn64_discover::tool_adapter::{
    ingest_tool_jsonl, AdapterLimits, BankInputIdentity, ToolAdapterExpectation, ToolLineageRef,
    ToolRunRole,
};
use serde_json::Value;
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        return Err("usage: gate_tool_jsonl JSONL [JSONL ...]".into());
    }

    for path in paths {
        let jsonl = fs::read_to_string(&path)?;
        let first = jsonl.lines().next().ok_or("empty JSONL stream")?;
        let header: Value = serde_json::from_str(first)?;
        if header.get("record").and_then(Value::as_str) != Some("header") {
            return Err(format!("{path}: first record is not a header").into());
        }
        let input: BankInputIdentity =
            serde_json::from_value(header.get("input").ok_or("missing input")?.clone())?;
        let role: ToolRunRole =
            serde_json::from_value(header.get("role").ok_or("missing role")?.clone())?;
        let lineage: Vec<ToolLineageRef> =
            serde_json::from_value(header.get("lineage").ok_or("missing lineage")?.clone())?;
        let output = ingest_tool_jsonl(
            &jsonl,
            &ToolAdapterExpectation {
                input,
                role,
                lineage,
                limits: AdapterLimits::default(),
            },
        )?;
        println!(
            "{}: source={} candidates={}",
            path,
            output.source().source_sha256.to_hex(),
            output.candidates().len()
        );
    }
    Ok(())
}
