use serde::{Deserialize, Serialize};

use crate::util::now_ms;

// ---------------------------------------------------------------------------
// Civil date utilities (dependency-free, Howard Hinnant's algorithms)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CivilDate {
    y: i64,
    m: u32,
    d: u32,
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

impl CivilDate {
    pub(crate) fn parse(value: &str) -> Option<CivilDate> {
        let value = value.trim();
        let mut parts = value.split('-');
        let y = parts.next()?.parse::<i64>().ok()?;
        let m = parts.next()?.parse::<u32>().ok()?;
        let d = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        if !(1900..=4000).contains(&y) || !(1..=12).contains(&m) {
            return None;
        }
        if d < 1 || d > days_in_month(y, m) {
            return None;
        }
        Some(CivilDate { y, m, d })
    }

    /// Days since the Unix epoch (1970-01-01).
    pub(crate) fn to_days(self) -> i64 {
        let y = self.y - if self.m <= 2 { 1 } else { 0 };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = if self.m > 2 { self.m - 3 } else { self.m + 9 } as i64;
        let doy = (153 * mp + 2) / 5 + self.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    pub(crate) fn from_days(z: i64) -> CivilDate {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        CivilDate {
            y: y + if m <= 2 { 1 } else { 0 },
            m,
            d,
        }
    }

    pub(crate) fn add_months(self, n: i64) -> CivilDate {
        let total = self.y * 12 + (self.m as i64 - 1) + n;
        let y = total.div_euclid(12);
        let m = (total.rem_euclid(12) + 1) as u32;
        let d = self.d.min(days_in_month(y, m));
        CivilDate { y, m, d }
    }

    pub(crate) fn format(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

fn today_civil() -> CivilDate {
    CivilDate::from_days((now_ms() / 86_400_000) as i64)
}

// ---------------------------------------------------------------------------
// Filing deadline analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeadlineMilestone {
    pub(crate) code: String,
    pub(crate) label: String,
    pub(crate) basis_date: String,
    pub(crate) due_date: String,
    pub(crate) days_remaining: i64,
    pub(crate) status: String,
    pub(crate) severity: String,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeadlineReport {
    pub(crate) today: String,
    pub(crate) milestones: Vec<DeadlineMilestone>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeadlineRequest {
    pub(crate) provisional_filing_date: Option<String>,
    pub(crate) public_disclosure_date: Option<String>,
    pub(crate) foreign_priority_date: Option<String>,
    pub(crate) today: Option<String>,
}

fn milestone(
    today: CivilDate,
    code: &str,
    label: &str,
    basis: CivilDate,
    months: i64,
    note: &str,
) -> DeadlineMilestone {
    let due = basis.add_months(months);
    let days_remaining = due.to_days() - today.to_days();
    let (status, severity) = if days_remaining < 0 {
        ("past", "blocker")
    } else if days_remaining <= 30 {
        ("due-soon", "warning")
    } else if days_remaining <= 90 {
        ("approaching", "warning")
    } else {
        ("ok", "info")
    };
    DeadlineMilestone {
        code: code.to_string(),
        label: label.to_string(),
        basis_date: basis.format(),
        due_date: due.format(),
        days_remaining,
        status: status.to_string(),
        severity: severity.to_string(),
        note: note.to_string(),
    }
}

pub(crate) fn analyze_deadlines(
    provisional_filing_date: Option<&str>,
    public_disclosure_date: Option<&str>,
    foreign_priority_date: Option<&str>,
    today_override: Option<&str>,
) -> DeadlineReport {
    let today = today_override
        .and_then(CivilDate::parse)
        .unwrap_or_else(today_civil);
    let mut milestones = Vec::new();
    let mut warnings = Vec::new();

    let provisional = provisional_filing_date.and_then(CivilDate::parse);
    if let Some(basis) = provisional {
        milestones.push(milestone(
            today,
            "nonprovisional-from-provisional",
            "Nonprovisional or PCT must claim provisional benefit",
            basis,
            12,
            "37 CFR 1.78: a provisional has 12 months of pendency and cannot be extended.",
        ));
        milestones.push(milestone(
            today,
            "provisional-restoration",
            "Restoration-of-priority outer limit",
            basis,
            14,
            "Benefit may be restored under 37 CFR 1.78 only within 14 months and only on petition.",
        ));
        milestones.push(milestone(
            today,
            "paris-convention-foreign",
            "Paris Convention foreign filing deadline",
            basis,
            12,
            "File foreign / PCT applications within 12 months to claim provisional priority.",
        ));
    }
    if provisional_filing_date.is_some() && provisional.is_none() {
        warnings.push("provisionalFilingDate is not a valid YYYY-MM-DD date.".to_string());
    }

    if let Some(basis) = foreign_priority_date.and_then(CivilDate::parse) {
        if provisional.is_none() {
            milestones.push(milestone(
                today,
                "paris-convention-foreign",
                "Paris Convention / PCT priority deadline",
                basis,
                12,
                "Downstream filings claiming this priority date are generally due within 12 months.",
            ));
        }
    } else if foreign_priority_date.is_some() {
        warnings.push("foreignPriorityDate is not a valid YYYY-MM-DD date.".to_string());
    }

    if let Some(basis) = public_disclosure_date.and_then(CivilDate::parse) {
        milestones.push(milestone(
            today,
            "us-grace-period-bar",
            "US one-year grace-period statutory bar (35 USC 102(b)(1))",
            basis,
            12,
            "A US application is generally barred 12 months after the inventor's public disclosure.",
        ));
        warnings.push(
            "A public disclosure was recorded: most non-US jurisdictions require absolute novelty, \
             so foreign rights may already be lost regardless of the US grace period."
                .to_string(),
        );
    } else if public_disclosure_date.is_some() {
        warnings.push("publicDisclosureDate is not a valid YYYY-MM-DD date.".to_string());
    }

    if milestones.is_empty() {
        warnings.push(
            "No filing/disclosure/priority dates were provided, so no deadlines were computed."
                .to_string(),
        );
    }

    milestones.sort_by_key(|item| item.days_remaining);
    DeadlineReport {
        today: today.format(),
        milestones,
        warnings,
    }
}
