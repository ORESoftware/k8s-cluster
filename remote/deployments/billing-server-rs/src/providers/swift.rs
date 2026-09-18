//! SWIFT / direct ACH — observer-mode (bank coordinates auth).
//!
//! There is no OAuth for SWIFT or for the legacy ACH file world. The tenant
//! provides their bank coordinates (account number, routing/SWIFT/IBAN, BIC,
//! intermediary bank, etc.) which we seal and use to identify their inbound /
//! outbound wires inside the BAI2 / MT940 / camt.053 statement files we pull
//! from the bank (or that the bank SFTPs to us).
//!
//! We do NOT initiate payments under Model A. Tenants initiate wires/ACH in
//! their own bank's portal; we observe via the statement feed.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BankCoordinates {
    pub bank_name: String,
    pub account_holder: String,
    pub account_number: String,
    pub routing_number: Option<String>, // US ABA
    pub swift_bic: Option<String>,      // International
    pub iban: Option<String>,           // Europe
    pub intermediary_bank: Option<String>,
    pub statement_feed_kind: StatementFeedKind,
    pub statement_feed_settings: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatementFeedKind {
    Bai2Sftp,
    Mt940Sftp,
    Camt053Sftp,
    ManualUpload,
    BankApi,
}
