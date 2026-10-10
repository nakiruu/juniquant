// SKETCH, not compiled, not normative. Crate home: execution/src/lots/updater.rs (scaffold.md).
// Updates tax_lots.parquet from today's fills.
// Runs at 16:20 ET before the ledger write.

use std::path::Path;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use uuid::Uuid;

use crate::data::parquet::{read_fills, read_lots, append_lot, update_lot};

pub struct LotUpdater {
    lots_path: String,
}

impl LotUpdater {
    pub fn new(lots_path: &str) -> Self {
        Self { lots_path: lots_path.to_string() }
    }

    /// Process all fills for the given date.
    pub fn process_fills(&self, date: NaiveDate) -> Result<(), LotError> {
        let fills = read_fills(date)?;

        for fill in fills {
            match fill.side.as_str() {
                "Buy" => self.open_lot(&fill)?,
                "Sell" => self.close_lots_fifo(&fill)?,
                _ => return Err(LotError::UnknownSide(fill.side)),
            }
        }

        Ok(())
    }

    fn open_lot(&self, fill: &Fill) -> Result<(), LotError> {
        let lot = TaxLot {
            lot_id: Uuid::new_v4().to_string(),
            ticker: fill.ticker.clone(),
            open_date: fill.filled_at.date_naive(),
            open_price: fill.price,
            open_quantity: fill.quantity,
            close_date: None,
            close_price: None,
            close_quantity: Decimal::ZERO,
            realized_pnl: None,
            holding_days: 0,
            order_id_open: fill.order_id.clone(),
            order_id_close: None,
        };
        append_lot(&self.lots_path, &lot)
    }

    fn close_lots_fifo(&self, fill: &Fill) -> Result<(), LotError> {
        let mut remaining = fill.quantity;
        let open_lots = read_open_lots_for_ticker(&self.lots_path, &fill.ticker)?;

        // FIFO: sort by open_date, then lot_id
        let mut sorted = open_lots;
        sorted.sort_by(|a, b| a.open_date.cmp(&b.open_date)
                                .then(a.lot_id.cmp(&b.lot_id)));

        for lot in sorted {
            if remaining.is_zero() { break; }

            let close_qty = remaining.min(lot.open_quantity);
            // Quantities are exact Decimals (fractional shares, amendment 1.2);
            // only the dollar P&L is converted to f64.
            let realized = (fill.price - lot.open_price)
                * close_qty.to_f64().ok_or(LotError::QuantityConversion)?;
            let holding_days = (fill.filled_at.date_naive() - lot.open_date).num_days() as i32;

            if close_qty == lot.open_quantity {
                // Full close: update in place
                update_lot(&self.lots_path, &lot.lot_id, |l| {
                    l.close_date = Some(fill.filled_at.date_naive());
                    l.close_price = Some(fill.price);
                    l.close_quantity = close_qty;
                    l.realized_pnl = Some(realized);
                    l.holding_days = holding_days;
                    l.order_id_close = Some(fill.order_id.clone());
                })?;
            } else {
                // Partial close: split
                // 1. Reduce the original lot
                update_lot(&self.lots_path, &lot.lot_id, |l| {
                    l.open_quantity -= close_qty;
                })?;

                // 2. Write a new row for the closed portion
                let closed = TaxLot {
                    lot_id: Uuid::new_v4().to_string(),
                    ticker: lot.ticker.clone(),
                    open_date: lot.open_date,
                    open_price: lot.open_price,
                    open_quantity: close_qty,
                    close_date: Some(fill.filled_at.date_naive()),
                    close_price: Some(fill.price),
                    close_quantity: close_qty,
                    realized_pnl: Some(realized),
                    holding_days,
                    order_id_open: lot.order_id_open.clone(),
                    order_id_close: Some(fill.order_id.clone()),
                };
                append_lot(&self.lots_path, &closed)?;
            }

            remaining -= close_qty;
        }

        if remaining > Decimal::ZERO {
            return Err(LotError::InsufficientLots {
                ticker: fill.ticker.clone(),
                remaining,
            });
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum LotError {
    UnknownSide(String),
    InsufficientLots { ticker: String, remaining: Decimal },
    QuantityConversion,
}

pub struct TaxLot {
    pub lot_id: String,
    pub ticker: String,
    pub open_date: NaiveDate,
    pub open_price: f64,
    pub open_quantity: Decimal,
    pub close_date: Option<NaiveDate>,
    pub close_price: Option<f64>,
    pub close_quantity: Decimal,
    pub realized_pnl: Option<f64>,
    pub holding_days: i32,
    pub order_id_open: String,
    pub order_id_close: Option<String>,
}

pub struct Fill {
    pub order_id: String,
    pub ticker: String,
    pub side: String,
    pub quantity: Decimal,
    pub price: f64,
    pub filled_at: chrono::DateTime<chrono::Utc>,
}

// Helper: read open lots for a ticker
fn read_open_lots_for_ticker(path: &str, ticker: &str) -> Result<Vec<TaxLot>, LotError> {
    let all = read_lots(path)?;
    Ok(all.into_iter()
          .filter(|l| l.ticker == ticker && l.close_date.is_none())
          .collect())
}