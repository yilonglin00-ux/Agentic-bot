//! Deterministic analysis of tabular data.
//!
//! WHY THIS IS NOT A PROMPT. Asking a 9B model for the mean of a column is
//! asking it to do arithmetic on tokens. It will produce a number, it will
//! produce it confidently, and it will sometimes be wrong - and a wrong number
//! in a report is worse than no number, because it looks like an answer. So
//! every figure Noki states about a table is computed here, in Rust, from the
//! actual rows. The model's job starts afterwards: explain what the figures
//! mean, in the user's language. It never gets to choose them.
//!
//! WHAT THE MODEL RECEIVES. A compact, already-computed summary: column types,
//! counts, the statistics below, the outliers with their row numbers, and the
//! trend direction. That is a few hundred tokens instead of a 50 MB CSV, which
//! is also why a large file stops being a context problem.
//!
//! THE STATISTICS ARE THE BORING ONES ON PURPOSE. Mean, median, quartiles,
//! standard deviation, min/max, missing counts, and a least-squares slope.
//! Outliers use Tukey's rule (1.5 x IQR), which needs no distributional
//! assumption - relevant because real spreadsheets are not normally distributed.

use serde::{Deserialize, Serialize};

/// What a column turned out to hold. Determined from the values, not a header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    Numeric,
    /// Looks like a date (ISO, German, or a bare year).
    Temporal,
    /// Few distinct values relative to the row count.
    Categorical,
    Text,
    Empty,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumericStats {
    pub count: usize,
    pub missing: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub q1: f64,
    pub q3: f64,
    pub std_dev: f64,
    pub sum: f64,
    /// Least-squares slope against row order. Positive = rising down the table.
    pub trend_slope: f64,
    /// Slope expressed as a share of the mean, so it can be talked about
    /// without knowing the unit.
    pub trend_pct_per_row: f64,
    /// Row numbers (1-based, excluding the header) outside 1.5 x IQR.
    pub outlier_rows: Vec<usize>,
    pub outlier_values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CategoryStats {
    pub distinct: usize,
    pub missing: usize,
    /// The most frequent values, largest first.
    pub top: Vec<(String, usize)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub index: usize,
    pub kind: ColumnKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric: Option<NumericStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categorical: Option<CategoryStats>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableAnalysis {
    pub rows: usize,
    pub columns: Vec<Column>,
    /// Rows that could not be parsed with the header's column count.
    pub malformed_rows: usize,
    pub delimiter: char,
    /// Whether the row count hit the parser's ceiling.
    pub truncated: bool,
}

/// Ceiling on rows read into memory for one analysis. A table larger than this
/// is analysed on its first slice, and the summary says so.
const MAX_ROWS: usize = 200_000;
/// A column with at most this share of distinct values is categorical.
const CATEGORICAL_RATIO: f64 = 0.5;

/// Picks the delimiter by counting candidates in the header line. Comma,
/// semicolon and tab all occur in real exports; German tools favour semicolon.
fn sniff_delimiter(first_line: &str) -> char {
    let candidates = [',', ';', '\t', '|'];
    candidates
        .iter()
        .copied()
        .max_by_key(|d| first_line.matches(*d).count())
        .filter(|d| first_line.matches(*d).count() > 0)
        .unwrap_or(',')
}

/// A CSV line split on `delim`, honouring double quotes and doubled quotes.
fn split_line(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            c if c == delim && !in_quotes => out.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    out.push(field);
    out.iter().map(|s| s.trim().to_owned()).collect()
}

/// Parses a number the way a spreadsheet writes one.
///
/// This is where most naive CSV analysis goes wrong: `1.234,56` is German for
/// `1234.56`, `1,234.56` is English for the same, `45%` is `45`, and `€1.200`
/// carries a symbol. Getting this wrong does not produce an error - it produces
/// a plausible, wrong mean.
pub fn parse_number(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let had_currency = s.chars().any(|c| matches!(c, '€' | '$' | '£'));
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, '€' | '$' | '£' | '%' | ' ' | '\u{00a0}' | '\''))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let has_comma = cleaned.contains(',');
    let has_dot = cleaned.contains('.');
    // `1.200` is genuinely ambiguous: 1.2 in English, 1200 in German. The tie
    // is broken by two signals that are individually weak but jointly reliable.
    // More than one dot can only be grouping ("1.200.000"), and a currency
    // amount written with exactly three decimals does not occur in practice,
    // so `€1.200` is 1200 while a bare `1.200` stays 1.2 - which is also what
    // `parse::<f64>` would do, so the default is the unsurprising one.
    let dot_is_grouping = has_dot
        && !has_comma
        && (cleaned.matches('.').count() > 1
            || (had_currency && cleaned.split('.').next_back().is_some_and(|t| t.len() == 3)));
    if dot_is_grouping {
        return cleaned
            .replace('.', "")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite());
    }
    let normalized = if has_comma && has_dot {
        // The rightmost separator is the decimal one.
        if cleaned.rfind(',') > cleaned.rfind('.') {
            cleaned.replace('.', "").replace(',', ".")
        } else {
            cleaned.replace(',', "")
        }
    } else if has_comma {
        // A single comma with exactly three trailing digits is a thousands
        // separator ("1,234"); otherwise it is a German decimal comma.
        let tail = cleaned.split(',').next_back().unwrap_or("");
        if tail.len() == 3 && cleaned.matches(',').count() == 1 && cleaned.len() > 4 {
            cleaned.replace(',', "")
        } else {
            cleaned.replace(',', ".")
        }
    } else {
        cleaned
    };
    normalized.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Recognises a bare year, a year-month, and a full date in the three
/// separators that actually occur (`2026-03-14`, `14.03.2026`, `03/14/2026`).
/// Year-month matters more than it looks: monthly exports are the most common
/// shape of business data, and misreading `2026-01` as text loses the axis.
fn looks_temporal(s: &str) -> bool {
    let t = s.trim();
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let is_year =
        |x: &str| x.len() == 4 && x.parse::<u32>().is_ok_and(|y| (1800..=2200).contains(&y));
    if t.len() == 4 {
        return is_year(t);
    }
    // YYYY-MM / YYYY.MM / YYYY/MM
    if digits == 6 && t.len() == 7 {
        let parts: Vec<&str> = t.split(['-', '.', '/']).collect();
        if parts.len() == 2 {
            return is_year(parts[0])
                && parts[1].parse::<u32>().is_ok_and(|m| (1..=12).contains(&m));
        }
    }
    let seps = t.matches('-').count() + t.matches('.').count() + t.matches('/').count();
    seps >= 2 && t.len() >= 8 && digits >= 4
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = p * (sorted.len() - 1) as f64;
    let low = pos.floor() as usize;
    let high = pos.ceil() as usize;
    if low == high {
        sorted[low]
    } else {
        sorted[low] + (pos - low as f64) * (sorted[high] - sorted[low])
    }
}

/// Least-squares slope of `values` against their position.
fn slope(values: &[(usize, f64)]) -> f64 {
    let n = values.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mean_x = values.iter().map(|(i, _)| *i as f64).sum::<f64>() / n;
    let mean_y = values.iter().map(|(_, v)| *v).sum::<f64>() / n;
    let num: f64 = values
        .iter()
        .map(|(i, v)| (*i as f64 - mean_x) * (v - mean_y))
        .sum();
    let den: f64 = values
        .iter()
        .map(|(i, _)| (*i as f64 - mean_x).powi(2))
        .sum();
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

fn numeric_stats(indexed: &[(usize, f64)], missing: usize) -> NumericStats {
    let mut sorted: Vec<f64> = indexed.iter().map(|(_, v)| *v).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = sorted.len();
    let sum: f64 = sorted.iter().sum();
    let mean = sum / count as f64;
    let variance = if count > 1 {
        sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (count - 1) as f64
    } else {
        0.0
    };
    let q1 = percentile(&sorted, 0.25);
    let q3 = percentile(&sorted, 0.75);
    let iqr = q3 - q1;
    // Tukey's rule. With a zero IQR every value is identical, so nothing is an
    // outlier - guarding this stops a constant column reporting every row.
    let (lo, hi) = if iqr > 0.0 {
        (q1 - 1.5 * iqr, q3 + 1.5 * iqr)
    } else {
        (f64::NEG_INFINITY, f64::INFINITY)
    };
    let mut outlier_rows = Vec::new();
    let mut outlier_values = Vec::new();
    for (row, v) in indexed {
        if *v < lo || *v > hi {
            outlier_rows.push(*row);
            outlier_values.push(*v);
        }
    }
    let s = slope(indexed);
    NumericStats {
        count,
        missing,
        min: sorted[0],
        max: sorted[count - 1],
        mean,
        median: percentile(&sorted, 0.5),
        q1,
        q3,
        std_dev: variance.sqrt(),
        sum,
        trend_slope: s,
        trend_pct_per_row: if mean != 0.0 { s / mean * 100.0 } else { 0.0 },
        outlier_rows,
        outlier_values,
    }
}

/// Analyses a delimited table. Never fails on messy input: a row that does not
/// fit the header is counted, not fatal.
pub fn analyze_table(text: &str) -> Result<TableAnalysis, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header_line = lines.next().ok_or("Die Datei enthält keine Zeilen.")?;
    let delimiter = sniff_delimiter(header_line);
    let header = split_line(header_line, delimiter);
    if header.len() < 2 {
        return Err("Die Datei sieht nicht wie eine Tabelle aus (nur eine Spalte).".into());
    }

    let mut cells: Vec<Vec<String>> = vec![Vec::new(); header.len()];
    let mut malformed_rows = 0usize;
    let mut rows = 0usize;
    let mut truncated = false;
    for line in lines {
        if rows >= MAX_ROWS {
            truncated = true;
            break;
        }
        let fields = split_line(line, delimiter);
        if fields.len() != header.len() {
            malformed_rows += 1;
            continue;
        }
        for (i, f) in fields.into_iter().enumerate() {
            cells[i].push(f);
        }
        rows += 1;
    }
    if rows == 0 {
        return Err("Die Tabelle enthält keine verwertbaren Datenzeilen.".into());
    }

    let columns = header
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let values = &cells[index];
            let missing = values.iter().filter(|v| v.is_empty()).count();
            let present: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
            if present.is_empty() {
                return Column {
                    name: name.clone(),
                    index,
                    kind: ColumnKind::Empty,
                    numeric: None,
                    categorical: None,
                };
            }
            let indexed: Vec<(usize, f64)> = values
                .iter()
                .enumerate()
                .filter_map(|(row, v)| parse_number(v).map(|n| (row + 1, n)))
                .collect();
            // A column counts as numeric only if nearly everything present
            // parses: one stray "n/a" should not turn a number column into
            // text, and one stray number should not make a text column numeric.
            let numeric_share = indexed.len() as f64 / present.len() as f64;
            let temporal_share =
                present.iter().filter(|v| looks_temporal(v)).count() as f64 / present.len() as f64;

            if temporal_share >= 0.9 {
                return Column {
                    name: name.clone(),
                    index,
                    kind: ColumnKind::Temporal,
                    numeric: None,
                    categorical: None,
                };
            }
            if numeric_share >= 0.9 && indexed.len() >= 2 {
                return Column {
                    name: name.clone(),
                    index,
                    kind: ColumnKind::Numeric,
                    numeric: Some(numeric_stats(&indexed, missing)),
                    categorical: None,
                };
            }
            let mut counts: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for v in &present {
                *counts.entry(v.as_str()).or_insert(0) += 1;
            }
            let distinct = counts.len();
            let mut top: Vec<(String, usize)> =
                counts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            top.truncate(5);
            let kind = if (distinct as f64 / present.len() as f64) <= CATEGORICAL_RATIO {
                ColumnKind::Categorical
            } else {
                ColumnKind::Text
            };
            Column {
                name: name.clone(),
                index,
                kind,
                numeric: None,
                categorical: Some(CategoryStats {
                    distinct,
                    missing,
                    top,
                }),
            }
        })
        .collect();

    Ok(TableAnalysis {
        rows,
        columns,
        malformed_rows,
        delimiter,
        truncated,
    })
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return "–".into();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The already-computed facts, as compact text for the model to interpret.
///
/// This is deliberately a FACTS block and not a narrative: the model is asked
/// to explain these numbers, which means it must not be given room to believe
/// it should recompute them.
pub fn facts_block(a: &TableAnalysis) -> String {
    let mut out = String::new();
    out.push_str("[BERECHNETE_FAKTEN – deterministisch aus der Datei berechnet, nicht schätzen, nicht neu rechnen]\n");
    out.push_str(&format!(
        "Zeilen: {} · Spalten: {} · Trennzeichen: {}\n",
        a.rows,
        a.columns.len(),
        match a.delimiter {
            '\t' => "Tab".to_string(),
            d => d.to_string(),
        }
    ));
    if a.malformed_rows > 0 {
        out.push_str(&format!(
            "Nicht auswertbare Zeilen (falsche Spaltenzahl): {}\n",
            a.malformed_rows
        ));
    }
    if a.truncated {
        out.push_str(&format!(
            "Hinweis: Nur die ersten {MAX_ROWS} Zeilen wurden ausgewertet.\n"
        ));
    }
    for c in &a.columns {
        match (&c.numeric, &c.categorical) {
            (Some(n), _) => {
                out.push_str(&format!(
                    "\nSpalte \"{}\" (numerisch, n={}{}):\n  Min {} · Q1 {} · Median {} · Q3 {} · Max {}\n  Mittelwert {} · Standardabweichung {} · Summe {}\n",
                    c.name,
                    n.count,
                    if n.missing > 0 { format!(", {} fehlend", n.missing) } else { String::new() },
                    num(n.min), num(n.q1), num(n.median), num(n.q3), num(n.max),
                    num(n.mean), num(n.std_dev), num(n.sum)
                ));
                let dir = if n.trend_pct_per_row > 0.5 {
                    "steigend"
                } else if n.trend_pct_per_row < -0.5 {
                    "fallend"
                } else {
                    "ohne klaren Trend"
                };
                out.push_str(&format!(
                    "  Verlauf über die Zeilen: {dir} ({} pro Zeile, {}% vom Mittelwert)\n",
                    num(n.trend_slope),
                    num(n.trend_pct_per_row)
                ));
                if n.outlier_rows.is_empty() {
                    out.push_str("  Ausreißer (1,5×IQR): keine\n");
                } else {
                    let shown: Vec<String> = n
                        .outlier_rows
                        .iter()
                        .zip(&n.outlier_values)
                        .take(8)
                        .map(|(r, v)| format!("Zeile {r}: {}", num(*v)))
                        .collect();
                    out.push_str(&format!(
                        "  Ausreißer (1,5×IQR), {} insgesamt: {}\n",
                        n.outlier_rows.len(),
                        shown.join("; ")
                    ));
                }
            }
            (None, Some(cat)) => {
                let top: Vec<String> = cat.top.iter().map(|(v, n)| format!("{v} ({n})")).collect();
                out.push_str(&format!(
                    "\nSpalte \"{}\" ({}, {} verschiedene Werte{}):\n  Häufigste: {}\n",
                    c.name,
                    if c.kind == ColumnKind::Categorical {
                        "kategorial"
                    } else {
                        "Text"
                    },
                    cat.distinct,
                    if cat.missing > 0 {
                        format!(", {} fehlend", cat.missing)
                    } else {
                        String::new()
                    },
                    top.join(", ")
                ));
            }
            _ => {
                out.push_str(&format!(
                    "\nSpalte \"{}\": {}\n",
                    c.name,
                    match c.kind {
                        ColumnKind::Temporal => "Datum/Zeit",
                        ColumnKind::Empty => "leer",
                        _ => "ohne Kennzahlen",
                    }
                ));
            }
        }
    }
    out.push_str("[/BERECHNETE_FAKTEN]\n");
    out
}

/// Whether a file should go through the table analyser at all.
pub fn is_tabular(file_type: &str, text: &str) -> bool {
    if matches!(file_type, "csv" | "tsv") {
        return true;
    }
    // A .txt export can still be a table. Require a consistent delimiter count
    // across the first few lines, so prose with one comma is not "tabular".
    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).take(5);
    let Some(first) = lines.next() else {
        return false;
    };
    let d = sniff_delimiter(first);
    let n = first.matches(d).count();
    n >= 1 && lines.clone().count() >= 2 && lines.all(|l| l.matches(d).count() == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALES: &str = "\
Monat,Umsatz,Region,Kunden
2026-01,1000,Nord,12
2026-02,1100,Sued,14
2026-03,1200,Nord,13
2026-04,1300,Sued,15
2026-05,1400,Nord,16
2026-06,9999,Sued,14
";

    #[test]
    fn columns_are_typed_from_their_values() {
        let a = analyze_table(SALES).unwrap();
        assert_eq!(a.rows, 6);
        assert_eq!(a.delimiter, ',');
        assert_eq!(a.columns[0].kind, ColumnKind::Temporal, "Monat");
        assert_eq!(a.columns[1].kind, ColumnKind::Numeric, "Umsatz");
        assert_eq!(a.columns[2].kind, ColumnKind::Categorical, "Region");
        assert_eq!(a.columns[3].kind, ColumnKind::Numeric, "Kunden");
    }

    #[test]
    fn statistics_are_exact_not_estimated() {
        let a = analyze_table(SALES).unwrap();
        let n = a.columns[1].numeric.as_ref().unwrap();
        assert_eq!(n.count, 6);
        assert_eq!(n.min, 1000.0);
        assert_eq!(n.max, 9999.0);
        assert_eq!(n.sum, 1000.0 + 1100.0 + 1200.0 + 1300.0 + 1400.0 + 9999.0);
        assert!((n.mean - n.sum / 6.0).abs() < 1e-9);
        assert_eq!(n.median, 1250.0);
    }

    #[test]
    fn the_planted_outlier_is_found_with_its_row() {
        let a = analyze_table(SALES).unwrap();
        let n = a.columns[1].numeric.as_ref().unwrap();
        assert_eq!(n.outlier_rows, vec![6], "row 6 holds 9999");
        assert_eq!(n.outlier_values, vec![9999.0]);
    }

    #[test]
    fn a_rising_column_is_reported_as_rising() {
        let a = analyze_table("x,y\n1,10\n2,20\n3,30\n4,40\n").unwrap();
        let n = a.columns[1].numeric.as_ref().unwrap();
        assert!((n.trend_slope - 10.0).abs() < 1e-9);
        assert!(n.trend_pct_per_row > 0.5);
        // A flat column has no trend and no outliers.
        let flat = analyze_table("x,y\n1,5\n2,5\n3,5\n4,5\n").unwrap();
        let f = flat.columns[1].numeric.as_ref().unwrap();
        assert_eq!(f.trend_slope, 0.0);
        assert!(
            f.outlier_rows.is_empty(),
            "a constant column has no outliers"
        );
    }

    #[test]
    fn german_and_english_number_formats_both_parse() {
        assert_eq!(parse_number("1.234,56"), Some(1234.56));
        assert_eq!(parse_number("1,234.56"), Some(1234.56));
        assert_eq!(parse_number("1234,5"), Some(1234.5));
        assert_eq!(parse_number("1,234"), Some(1234.0));
        assert_eq!(parse_number("0,5"), Some(0.5));
        assert_eq!(parse_number("45%"), Some(45.0));
        // The ambiguous `1.200` case, decided explicitly rather than by luck:
        // grouping when a currency or a second dot says so, decimal otherwise.
        assert_eq!(parse_number("€1.200"), Some(1200.0));
        assert_eq!(parse_number("1.200.000"), Some(1_200_000.0));
        assert_eq!(parse_number("1.200"), Some(1.2), "a bare dot stays decimal");
        assert_eq!(parse_number("€1.20"), Some(1.20), "two decimals are money");
        assert_eq!(parse_number("-12,5"), Some(-12.5));
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("n/a"), None);
        assert_eq!(parse_number("Nord"), None);
    }

    #[test]
    fn a_german_export_with_semicolons_is_read_correctly() {
        let csv = "Monat;Umsatz\nJan;1.000,50\nFeb;2.000,75\n";
        let a = analyze_table(csv).unwrap();
        assert_eq!(a.delimiter, ';');
        let n = a.columns[1].numeric.as_ref().unwrap();
        assert!((n.sum - 3001.25).abs() < 1e-9, "sum was {}", n.sum);
    }

    #[test]
    fn quoted_fields_with_delimiters_stay_one_field() {
        let csv = "name,note\n\"Meier, Anna\",ok\n\"Say \"\"hi\"\"\",fine\n";
        let a = analyze_table(csv).unwrap();
        assert_eq!(a.rows, 2);
        assert_eq!(a.malformed_rows, 0);
        let cat = a.columns[0].categorical.as_ref().unwrap();
        assert!(
            cat.top.iter().any(|(v, _)| v == "Meier, Anna"),
            "got {:?}",
            cat.top
        );
        assert!(
            cat.top.iter().any(|(v, _)| v == "Say \"hi\""),
            "got {:?}",
            cat.top
        );
    }

    #[test]
    fn messy_rows_are_counted_and_never_fatal() {
        let csv = "a,b\n1,2\nbroken\n3,4\n5,6,7\n";
        let a = analyze_table(csv).unwrap();
        assert_eq!(a.rows, 2);
        assert_eq!(a.malformed_rows, 2);
    }

    #[test]
    fn a_mostly_numeric_column_with_one_na_stays_numeric() {
        let csv = "a,b\n1,10\n2,20\n3,30\n4,40\n5,50\n6,60\n7,70\n8,80\n9,90\n10,n/a\n";
        let a = analyze_table(csv).unwrap();
        assert_eq!(a.columns[1].kind, ColumnKind::Numeric);
        assert_eq!(a.columns[1].numeric.as_ref().unwrap().count, 9);
    }

    #[test]
    fn the_facts_block_states_the_numbers_and_forbids_recomputation() {
        let a = analyze_table(SALES).unwrap();
        let f = facts_block(&a);
        assert!(f.contains("BERECHNETE_FAKTEN"));
        assert!(f.contains("nicht neu rechnen"));
        assert!(
            f.contains("Zeile 6: 9999"),
            "the outlier must be named:\n{f}"
        );
        assert!(f.contains("Median 1250"));
        assert!(f.contains("Nord (3)"), "category counts missing:\n{f}");
        // It stays small enough to be cheap context.
        assert!(f.len() < 4_000, "facts block is {} bytes", f.len());
    }

    #[test]
    fn a_big_table_summarises_to_a_small_context() {
        let mut csv = String::from("i,v\n");
        for i in 1..=50_000 {
            csv.push_str(&format!("{i},{}\n", i % 977));
        }
        let a = analyze_table(&csv).unwrap();
        assert_eq!(a.rows, 50_000);
        let f = facts_block(&a);
        // 50k rows of source -> a few hundred bytes of evidence.
        assert!(f.len() < 3_000, "facts block is {} bytes", f.len());
        assert!(csv.len() > 300_000);
    }

    #[test]
    fn non_tables_are_rejected_rather_than_mis_analysed() {
        assert!(is_tabular("csv", "a,b\n1,2\n"));
        assert!(is_tabular("txt", "a,b\n1,2\n3,4\n"));
        // Prose with a comma is not a table.
        let prose = "Hallo, wie geht es dir?\nMir geht es gut.\nUnd dir so?\n";
        assert!(!is_tabular("txt", prose));
        assert!(analyze_table("nur eine Spalte\nZeile\n").is_err());
        assert!(analyze_table("").is_err());
        assert!(analyze_table("a,b\n").is_err());
    }
}
