//! Bounded, cached CSV/TSV preview. Input comes from the existing text loader.
use std::{path::Path, sync::Arc};

const MAX_BYTES: usize = 256 * 1024;
const MAX_ROWS: usize = 500;
const MAX_COLUMNS: usize = 64;

pub fn delimiter(path: &Path) -> Option<u8> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "csv" => Some(b','),
        "tsv" => Some(b'\t'),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct Table {
    rows: Vec<Vec<String>>,
    columns: usize,
    limited: bool,
    error: Option<String>,
}

fn parse(input: &str, delimiter: u8) -> Table {
    let mut end = input.len().min(MAX_BYTES);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut table = Table {
        rows: Vec::new(),
        columns: 0,
        limited: end < input.len(),
        error: None,
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(input[..end].trim_start_matches('\u{feff}').as_bytes());
    for result in reader.records() {
        if table.rows.len() == MAX_ROWS {
            table.limited = true;
            break;
        }
        match result {
            Ok(record) => {
                table.limited |= record.len() > MAX_COLUMNS;
                let row: Vec<String> = record.iter().take(MAX_COLUMNS).map(str::to_owned).collect();
                table.columns = table.columns.max(row.len());
                table.rows.push(row);
            }
            Err(error) => {
                table.error = Some(error.to_string());
                break;
            }
        }
    }
    table
}

#[derive(Clone)]
struct CachedTable {
    content: Arc<str>,
    delimiter: u8,
    table: Arc<Table>,
}

pub fn render(ui: &mut egui::Ui, content: &Arc<str>, delimiter: u8, side: &str) {
    let id = egui::Id::new(("table_preview", side));
    let cached = ui.ctx().data(|data| data.get_temp::<CachedTable>(id));
    let changed = cached.as_ref().is_none_or(|cached| {
        cached.delimiter != delimiter || !Arc::ptr_eq(&cached.content, content)
    });
    let table = match cached {
        Some(cached) if cached.delimiter == delimiter && Arc::ptr_eq(&cached.content, content) => {
            cached.table
        }
        _ => {
            let table = Arc::new(parse(content, delimiter));
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    id,
                    CachedTable {
                        content: content.clone(),
                        delimiter,
                        table: table.clone(),
                    },
                )
            });
            table
        }
    };
    ui.small(format!(
        "{} rows · {} columns",
        table.rows.len(),
        table.columns
    ));
    if table.limited {
        ui.small("Partial preview: limited to 500 rows, 64 columns and 256 KiB. Use Source for loaded text.");
    }
    if let Some(error) = &table.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if table.rows.is_empty() {
        ui.label("No table rows.");
        return;
    }
    let mut scroll = egui::ScrollArea::both().id_salt(id.with("scroll"));
    if changed {
        scroll = scroll.scroll_offset(egui::Vec2::ZERO);
    }
    scroll.show_rows(ui, 22.0, table.rows.len(), |ui, range| {
        egui::Grid::new(id.with("grid"))
            .striped(true)
            .min_col_width(100.0)
            .show(ui, |ui| {
                for row in range {
                    ui.label(egui::RichText::new((row + 1).to_string()).weak());
                    for column in 0..table.columns {
                        let value = table.rows[row]
                            .get(column)
                            .map(String::as_str)
                            .unwrap_or("");
                        ui.add_sized(
                            [160.0, 22.0],
                            egui::Label::new(value).truncate().selectable(true),
                        )
                        .on_hover_text(value);
                    }
                    ui.end_row();
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_multiline_unicode_and_ragged_rows() {
        let table = parse(
            "\u{feff}name,note\r\nёж,\"one, two\nthree\"\r\nlast\r\n",
            b',',
        );
        assert_eq!(table.rows[1], ["ёж", "one, two\nthree"]);
        assert_eq!(table.rows[2], ["last"]);
        assert_eq!(table.columns, 2);
        assert!(!table.limited);
    }
    #[test]
    fn tsv_preserves_empty_cells_and_escaped_quotes() {
        let table = parse("a\t\t\"say \"\"hi\"\"\"\n", b'\t');
        assert_eq!(table.rows[0], ["a", "", "say \"hi\""]);
        assert_eq!(delimiter(Path::new("DATA.TSV")), Some(b'\t'));
    }
    #[test]
    fn formulas_are_plain_text_and_empty_files_have_no_rows() {
        assert_eq!(
            parse("=SUM(A1:A2),+cmd,-1,@name", b',').rows[0],
            ["=SUM(A1:A2)", "+cmd", "-1", "@name"]
        );
        assert!(parse("", b',').rows.is_empty());
    }

    #[test]
    fn bounds_rows_columns_and_unicode_bytes() {
        let table = parse(&"a,b\n".repeat(501), b',');
        assert_eq!(table.rows.len(), MAX_ROWS);
        assert!(table.limited);
        let table = parse(&vec!["a"; 65].join(","), b',');
        assert_eq!(table.columns, MAX_COLUMNS);
        assert!(table.limited);
        let table = parse(&"ё".repeat(MAX_BYTES), b',');
        assert!(table.limited);
        assert!(table.rows[0][0].len() <= MAX_BYTES);
    }
}
