//! Golden vectors: `schemas/fixtures/symbol_normalization.csv` and
//! `schemas/fixtures/symbol_render.csv` (UNIVERSE.md §3). The fixture files are the authority;
//! every row must be reproduced exactly, including the error kind strings.

use std::path::PathBuf;
use std::str::FromStr;

use symbols::{Canon, Vendor, is_tradable, normalize, render_str};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../schemas/fixtures")
        .join(name)
}

#[allow(
    clippy::unwrap_used,
    reason = "test helper: an unreadable fixture must fail the test"
)]
fn read_rows(name: &str, columns: &[&str]) -> Vec<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .trim(csv::Trim::None)
        .from_path(fixture(name))
        .unwrap();
    let headers: Vec<String> = reader
        .headers()
        .unwrap()
        .iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(headers, columns, "{name}: unexpected header");
    reader
        .records()
        .map(|r| r.unwrap().iter().map(str::to_owned).collect::<Vec<_>>())
        .collect()
}

#[test]
fn normalization_fixture_rows() {
    let rows = read_rows(
        "symbol_normalization.csv",
        &["input", "normalized", "error", "is_tradable_input", "note"],
    );
    assert!(rows.len() >= 41, "fixture shrank: {} rows", rows.len());
    let mut failures = Vec::new();
    for row in &rows {
        let [input, normalized, error, tradable, note] = row.as_slice() else {
            unreachable!("row width checked by the csv reader")
        };
        let got = normalize(input);
        let (got_norm, got_err) = match &got {
            Ok(c) => (c.to_string(), String::new()),
            Err(e) => (String::new(), e.kind().to_owned()),
        };
        if &got_norm != normalized || &got_err != error {
            failures.push(format!(
                "normalize({input:?}) = ({got_norm:?}, {got_err:?}), want ({normalized:?}, {error:?}) [{note}]"
            ));
        }
        let want_tradable = match tradable.as_str() {
            "true" => true,
            "false" => false,
            other => unreachable!("bad is_tradable_input {other:?}"),
        };
        if is_tradable(input) != want_tradable {
            failures.push(format!(
                "is_tradable({input:?}) = {}, want {want_tradable} [{note}]",
                !want_tradable
            ));
        }
        // Strict parsing agrees with is_tradable.
        if Canon::from_str(input).is_ok() != want_tradable {
            failures.push(format!(
                "Canon::from_str({input:?}) disagrees with is_tradable"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn render_fixture_rows() {
    let rows = read_rows(
        "symbol_render.csv",
        &["canonical", "vendor", "rendered", "error", "note"],
    );
    assert!(rows.len() >= 11, "fixture shrank: {} rows", rows.len());
    let mut failures = Vec::new();
    for row in &rows {
        let [canonical, vendor, rendered, error, note] = row.as_slice() else {
            unreachable!("row width checked by the csv reader")
        };
        let vendor = Vendor::from_str(vendor).unwrap();
        let (got_out, got_err) = match render_str(canonical, vendor) {
            Ok(s) => (s, String::new()),
            Err(e) => (String::new(), e.kind().to_owned()),
        };
        if &got_out != rendered || &got_err != error {
            failures.push(format!(
                "render({canonical:?}, {vendor}) = ({got_out:?}, {got_err:?}), want ({rendered:?}, {error:?}) [{note}]"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
