//! `Canon`, `Cik`, `SupersededTicker` and `Vendor` value types.

use std::str::FromStr;

use symbols::{Canon, Cik, RenderError, SupersededTicker, Vendor, render};

#[test]
fn canon_accessors() {
    let c = Canon::from_str("BRK.B").unwrap();
    assert_eq!(c.base(), "BRK");
    assert_eq!(c.class(), Some('B'));
    assert_eq!(c.as_str(), "BRK.B");
    assert_eq!(c.to_string(), "BRK.B");
    let g = Canon::from_str("GOOGL").unwrap();
    assert_eq!(g.base(), "GOOGL");
    assert_eq!(g.class(), None);
}

#[test]
fn canon_from_str_is_strict() {
    for bad in [
        "brk.b",
        " AAPL",
        "BRK-B",
        "",
        "ABC~0000012345",
        "ABCDEFG",
        "A.BC",
    ] {
        let err = Canon::from_str(bad).unwrap_err();
        assert_eq!(err.kind(), "not_canonical", "{bad:?}");
    }
}

#[test]
fn canon_serde_validates() {
    #[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq)]
    struct Row {
        ticker: Canon,
    }
    let row: Row = toml::from_str("ticker = \"BRK.B\"").unwrap();
    assert_eq!(row.ticker.as_str(), "BRK.B");
    assert_eq!(toml::to_string(&row).unwrap().trim(), "ticker = \"BRK.B\"");
    assert!(toml::from_str::<Row>("ticker = \"brk-b\"").is_err());
    assert!(toml::from_str::<Row>("ticker = \"ABC~0000012345\"").is_err());
}

#[test]
fn cik_pads_and_validates() {
    assert_eq!(Cik::from_str("320193").unwrap().to_string(), "0000320193");
    assert_eq!(
        Cik::from_str("0000320193").unwrap().to_string(),
        "0000320193"
    );
    assert_eq!(Cik::new(1_067_983).unwrap().as_u64(), 1_067_983);
    assert_eq!(Cik::new(9_999_999_999).unwrap().to_string(), "9999999999");
    for bad in [
        "",
        "12345678901",
        "12a",
        " 123",
        "-1",
        "0",
        "0000000000",
        "+12",
    ] {
        assert!(Cik::from_str(bad).is_err(), "{bad:?}");
    }
    assert!(Cik::new(0).is_err());
    assert!(Cik::new(10_000_000_000).is_err());
}

#[test]
fn superseded_ticker_renders_and_parses() {
    let s = SupersededTicker::new(
        &Canon::from_str("ABC").unwrap(),
        Cik::from_str("12345").unwrap(),
    );
    assert_eq!(s.to_string(), "ABC~0000012345");
    assert_eq!(s.symbol(), "ABC");
    assert_eq!(s.cik().to_string(), "0000012345");
    assert_eq!(SupersededTicker::from_str("ABC~0000012345").unwrap(), s);
    let class = SupersededTicker::from_str("BRK.B~0001067983").unwrap();
    assert_eq!(class.symbol(), "BRK.B");
    for bad in [
        "ABC",
        "abc~0000012345",
        "ABC~12345",
        "ABC~00000123456",
        "ABC~~0000012345",
        "~0000012345",
        "ABC~0000012345 ",
        "ABC~0000000000",
    ] {
        assert!(SupersededTicker::from_str(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn vendor_names_round_trip() {
    for v in [Vendor::Yahoo, Vendor::Sec, Vendor::Schwab, Vendor::IShares] {
        assert_eq!(Vendor::from_str(v.as_str()).unwrap(), v);
        assert_eq!(v.to_string(), v.as_str());
    }
    assert!(Vendor::from_str("Yahoo").is_err());
    assert!(Vendor::from_str("ibkr").is_err());
}

#[test]
fn render_errors() {
    let brkb = Canon::from_str("BRK.B").unwrap();
    assert_eq!(
        render(&brkb, Vendor::Schwab),
        Err(RenderError::UnverifiedVendor {
            vendor: Vendor::Schwab,
            symbol: brkb.clone()
        })
    );
    assert_eq!(
        render(&brkb, Vendor::IShares).unwrap_err().kind(),
        "input_only"
    );
    assert_eq!(render(&brkb, Vendor::Yahoo).unwrap(), "BRK-B");
}
