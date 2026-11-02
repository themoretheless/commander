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

#[derive(Clone)]
struct DialectChoice {
    content: Arc<str>,
    file_delimiter: u8,
    selected: Option<u8>,
    detection_error: bool,
}
/// Choose only a unique consistent multi-column dialect in a bounded sample.
/// CSV parsing, rather than counting raw separators, respects quoted newlines.
fn detect_delimiter(input: &str) -> Option<u8> {
    let mut end = input.len().min(MAX_BYTES);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let sample = input[..end].trim_start_matches('\u{feff}');
    let mut winner = None;
    for delimiter in *b",\t;|" {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(true)
            .from_reader(sample.as_bytes());
        let mut width = None;
        let mut count = 0;
        let mut valid = true;
        for record in reader.records().take(32) {
            let Ok(record) = record else {
                valid = false;
                break;
            };
            let columns = record.len();
            if !(2..=MAX_COLUMNS).contains(&columns) || width.is_some_and(|width| width != columns)
            {
                valid = false;
                break;
            }
            width = Some(columns);
            count += 1;
        }
        if valid && count >= 2 {
            if winner.is_some() {
                return None;
            }
            winner = Some(delimiter);
        }
    }
    winner
}

fn delimiter_label(delimiter: u8) -> &'static str {
    match delimiter {
        b',' => "Comma",
        b'\t' => "Tab",
        b';' => "Semicolon",
        b'|' => "Pipe",
        _ => "Custom",
    }
}

#[derive(Clone, Default)]
struct View {
    first_heading: bool,
    numeric_sort: bool,
    sort: Option<(usize, bool)>,
    filter: RowFilter,
    filter_error: Option<String>,
    order_ready: bool,
    order: Arc<Vec<usize>>,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct RowFilter {
    query: String,
    fuzzy: bool,
    regex: bool,
    whole_words: bool,
    match_case: bool,
    smart_case: bool,
    column: Option<usize>,
    exclude: bool,
}

impl RowFilter {
    fn retain(&self, table: &Table, order: &mut Vec<usize>) -> Result<(), String> {
        // UI and non-UI callers share the query bound; overlong input fails closed.
        if self.query.chars().take(257).count() > 256 {
            order.clear();
            return Err("Table filter queries are limited to 256 characters".into());
        }
        if self.column.is_some_and(|column| column >= table.columns) {
            order.clear();
            return Err("The selected table column is unavailable".into());
        }
        let query = self.query.trim();
        if query.is_empty() {
            return Ok(());
        }
        let sensitive =
            crate::preview_find::search_case_sensitive(query, self.match_case, self.smart_case);
        let pattern = if self.regex || self.whole_words {
            match crate::preview_find::compile_search_pattern(
                query,
                sensitive,
                self.regex,
                self.whole_words,
            ) {
                Ok(pattern) => Some(pattern),
                Err(error) => {
                    order.clear();
                    return Err(format!("Invalid table filter: {error}"));
                }
            }
        } else {
            None
        };
        let lower = (!sensitive && !self.fuzzy && pattern.is_none()).then(|| query.to_lowercase());
        order.retain(|&row| {
            let matched = table.rows[row].iter().enumerate().any(|(column, cell)| {
                if self.column.is_some_and(|chosen| chosen != column) {
                    return false;
                }
                if let Some(pattern) = &pattern {
                    pattern.is_match(cell)
                } else if self.fuzzy {
                    crate::fuzzy::is_match_case(query, cell, sensitive)
                } else if let Some(query) = &lower {
                    cell.to_lowercase().contains(query)
                } else {
                    cell.contains(query)
                }
            });
            matched != self.exclude
        });
        Ok(())
    }
}

#[cfg(feature = "visual-qa")]
pub fn filter_is(ctx: &egui::Context, side: &str, query: &str, fuzzy: bool) -> bool {
    ctx.data(|data| data.get_temp::<View>(egui::Id::new(("table_preview", side)).with("view")))
        .is_some_and(|view| view.filter.query == query && view.filter.fuzzy == fuzzy)
}

#[cfg(feature = "visual-qa")]
pub fn filter_snapshot(ctx: &egui::Context, side: &str) -> Option<serde_json::Value> {
    ctx.data(|data| data.get_temp::<View>(egui::Id::new(("table_preview", side)).with("view")))
        .map(|view| serde_json::json!({"query":view.filter.query,"fuzzy":view.filter.fuzzy,"regex":view.filter.regex,"whole_words":view.filter.whole_words,"column":view.filter.column,"exclude":view.filter.exclude,"error":view.filter_error,"rows":view.order.as_ref(),"heading":view.first_heading,"numeric_sort":view.numeric_sort,"sort":view.sort}))
}

/// Exact finite decimal/scientific notation, bounded without expanding powers.
/// Canonical significant digits share an implied decimal position `magnitude`.
#[derive(Debug, PartialEq, Eq)]
struct DecimalKey {
    negative: bool,
    digits: Vec<u8>,
    magnitude: i64,
}
impl DecimalKey {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.len() > 4096 {
            return None;
        }
        let (negative, text) = if let Some(rest) = text.strip_prefix('-') {
            (true, rest)
        } else {
            (false, text.strip_prefix('+').unwrap_or(text))
        };
        let (mantissa, exponent) = match text.find(['e', 'E']) {
            Some(index) => (&text[..index], text[index + 1..].parse::<i32>().ok()?),
            None => (text, 0),
        };
        if !(-1_000_000..=1_000_000).contains(&exponent) {
            return None;
        }
        let mut digits = Vec::with_capacity(mantissa.len());
        let mut fractional = 0_i64;
        let mut point = false;
        for byte in mantissa.bytes() {
            match byte {
                b'0'..=b'9' => {
                    digits.push(byte);
                    if point {
                        fractional += 1;
                    }
                }
                b'.' if !point => point = true,
                _ => return None,
            }
        }
        if digits.is_empty() {
            return None;
        }
        let Some(first) = digits.iter().position(|&digit| digit != b'0') else {
            return Some(Self {
                negative: false,
                digits: Vec::new(),
                magnitude: 0,
            });
        };
        let magnitude = (digits.len() - first) as i64 - fractional + i64::from(exponent);
        let end = digits.iter().rposition(|&digit| digit != b'0')? + 1;
        Some(Self {
            negative,
            digits: digits[first..end].to_vec(),
            magnitude,
        })
    }
    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if self.negative != other.negative {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let order = match (self.digits.is_empty(), other.digits.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => self
                .magnitude
                .cmp(&other.magnitude)
                .then_with(|| self.digits.cmp(&other.digits)),
        };
        if self.negative {
            order.reverse()
        } else {
            order
        }
    }
}

#[cfg(test)]
fn ordered_rows(table: &Table, heading: bool, sort: Option<(usize, bool)>) -> Vec<usize> {
    ordered_rows_with_policy(table, heading, sort, false)
}
fn ordered_rows_with_policy(
    table: &Table,
    heading: bool,
    sort: Option<(usize, bool)>,
    numeric: bool,
) -> Vec<usize> {
    use std::cmp::Ordering;
    let mut order: Vec<_> =
        (usize::from(heading && !table.rows.is_empty())..table.rows.len()).collect();
    if let Some((column, descending)) = sort.filter(|(column, _)| *column < table.columns) {
        let numbers: Vec<_> = if numeric {
            table
                .rows
                .iter()
                .map(|row| row.get(column).and_then(|cell| DecimalKey::parse(cell)))
                .collect()
        } else {
            Vec::new()
        };
        order.sort_by(|&a, &b| {
            let comparison = if numeric {
                match (&numbers[a], &numbers[b]) {
                    (Some(a), Some(b)) => {
                        let cmp = a.compare(b);
                        if descending { cmp.reverse() } else { cmp }
                    }
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                }
            } else {
                let value = |row: usize| table.rows[row].get(column).map_or("", String::as_str);
                let cmp = crate::natural_sort::compare(value(a), value(b));
                if descending { cmp.reverse() } else { cmp }
            };
            comparison.then_with(|| a.cmp(&b))
        });
    }
    order
}

fn choose_column(view: &mut View, column: usize) {
    view.sort = match view.sort {
        Some((current, false)) if current == column => Some((column, true)),
        Some((current, true)) if current == column => None,
        _ => Some((column, false)),
    };
}

#[cfg(any(test, feature = "visual-qa"))]
pub fn shown_order(ctx: &egui::Context, side: &str) -> Option<(bool, Vec<usize>)> {
    ctx.data(|data| data.get_temp::<View>(egui::Id::new(("table_preview", side)).with("view")))
        .map(|view| (view.first_heading, view.order.as_ref().clone()))
}

/// Serialize exactly the current loaded records, with optional headings.
/// CSV quoting preserves cell contents; this exports records, not source bytes.
fn export_rows(table: &Table, view: &View, delimiter: u8) -> Result<String, String> {
    const MAX_EXPORT_BYTES: usize = 1024 * 1024;
    if !matches!(delimiter, b',' | b'\t' | b';' | b'|') {
        return Err("Choose comma, tab, semicolon or pipe for table export".into());
    }
    if !view.order_ready || view.order.is_empty() {
        return Err("No visible data records to copy".into());
    }
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    let heading = view.first_heading.then_some(0);
    for row in heading.into_iter().chain(view.order.iter().copied()) {
        let cells = table
            .rows
            .get(row)
            .ok_or("Table view no longer matches its records")?;
        if view.first_heading && row == 0 && view.order.contains(&0) {
            return Err("Heading cannot also be a visible data record".into());
        }
        writer
            .write_record(cells)
            .map_err(|error| error.to_string())?;
        if writer.get_ref().len() > MAX_EXPORT_BYTES {
            return Err("Table export exceeds 1 MiB".into());
        }
    }
    let bytes = writer.into_inner().map_err(|error| error.to_string())?;
    if bytes.len() > MAX_EXPORT_BYTES {
        return Err("Table export exceeds 1 MiB".into());
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

pub fn render(
    ui: &mut egui::Ui,
    content: &Arc<str>,
    file_delimiter: u8,
    side: &str,
) -> Option<String> {
    let id = egui::Id::new(("table_preview", side));
    let mut dialect = ui
        .ctx()
        .data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
        .filter(|choice| {
            choice.file_delimiter == file_delimiter && Arc::ptr_eq(&choice.content, content)
        })
        .unwrap_or_else(|| DialectChoice {
            content: content.clone(),
            file_delimiter,
            selected: None,
            detection_error: false,
        });
    ui.horizontal_wrapped(|ui| {
        ui.label("Delimiter");
        let previous=dialect.selected;
        let combo=egui::ComboBox::from_id_salt(id.with("delimiter-menu"))
            .selected_text(dialect.selected.map(delimiter_label).unwrap_or("File default"))
            .show_ui(ui,|ui| {
                for (value,label) in [(None,"File default"),(Some(b','),"Comma"),(Some(b'\t'),"Tab"),(Some(b';'),"Semicolon"),(Some(b'|'),"Pipe")] {
                    let option=ui.selectable_value(&mut dialect.selected,value,label);
                    if option.clicked() {dialect.detection_error=false;}
                    #[cfg(any(test,feature="visual-qa"))]
                    ui.ctx().data_mut(|data|data.insert_temp(id.with(("delimiter-option",label)),option.rect));
                    #[cfg(not(any(test,feature="visual-qa")))]
                    let _=option;
                }
            });
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|data|data.insert_temp(id.with("delimiter"),combo.response.rect));
        combo.response.on_hover_text("Reparse loaded text with the chosen separator. Changing the effective separator resets headings, filtering and sorting. File default uses comma for CSV and tab for TSV.");
        if dialect.selected!=previous {dialect.detection_error=false;}
        let detect=ui.button("Detect delimiter").on_hover_text("Inspect at most 32 loaded CSV records. Require at least two records with the same 2–64 columns and exactly one matching separator. Ambiguous samples keep the current choice.");
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|data|data.insert_temp(id.with("detect-delimiter"),detect.rect));
        if detect.clicked() {
            if let Some(delimiter)=detect_delimiter(content) {dialect.selected=Some(delimiter);dialect.detection_error=false;}
            else {dialect.detection_error=true;}
        }
        ui.small(format!("Using {}",delimiter_label(dialect.selected.unwrap_or(file_delimiter))));
    });
    if dialect.detection_error {
        ui.colored_label(ui.visuals().error_fg_color,"No unique delimiter found in the sampled records; choose one manually. Current table is unchanged.");
    }
    let delimiter = dialect.selected.unwrap_or(file_delimiter);
    ui.ctx()
        .data_mut(|data| data.insert_temp(id.with("dialect"), dialect));
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
        ui.ctx().data_mut(|data| {
            data.remove_temp::<View>(id.with("view"));
        });
        ui.label("No table rows.");
        return None;
    }
    let mut view = if changed {
        View::default()
    } else {
        ui.ctx()
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap_or_default()
    };
    let before = (
        view.first_heading,
        view.sort,
        view.numeric_sort,
        view.filter.clone(),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let heading = ui.checkbox(&mut view.first_heading, "First row is headings")
            .on_hover_text("Keep record 1 above the scrolling data. Disable for files without headings; no row is discarded.");
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(id.with("heading"), heading.rect));
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = heading;
        let numeric = ui.checkbox(&mut view.numeric_sort, "Numeric sort").on_hover_text("Exact signed decimal/scientific numbers, up to 4096 bytes and exponents ±1000000. Empty, missing and non-numeric cells stay last in both directions. Original cell text is preserved.");
        #[cfg(any(test,feature="visual-qa"))]
        ui.ctx().data_mut(|data|data.insert_temp(id.with("numeric-sort"),numeric.rect));
        #[cfg(not(any(test,feature="visual-qa")))]
        let _=numeric;
        if ui.add_enabled(view.sort.is_some(), egui::Button::new("Original order")).clicked() {
            view.sort = None;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label("Filter rows");
        let query = ui.add(egui::TextEdit::singleline(&mut view.filter.query)
            .id(id.with("filter-input")).char_limit(256).desired_width(180.0)
            .hint_text("Text in chosen columns"))
            .on_hover_text("Match within one cell of the selected columns. Outer spaces are trimmed; inner spaces are literal. Only loaded records are searched; headings stay visible.");
        query.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, query.enabled(), "Table row filter"));
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(id.with("filter"), query.rect));
        let clear = ui.add_enabled(!view.filter.query.is_empty(), egui::Button::new("Clear filter"));
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(id.with("clear-filter"), clear.rect));
        if clear.clicked() { view.filter.query.clear(); query.request_focus(); }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let fuzzy = ui.checkbox(&mut view.filter.fuzzy, "Fuzzy")
            .on_hover_text("Match query characters in order with gaps inside one cell; preserves the chosen column/source order.");
        if fuzzy.changed() && view.filter.fuzzy { view.filter.regex = false; view.filter.whole_words = false; }
        let regex = ui.checkbox(&mut view.filter.regex, "Regex");
        if regex.changed() && view.filter.regex { view.filter.fuzzy = false; }
        let words = ui.checkbox(&mut view.filter.whole_words, "Whole words")
            .on_hover_text("Unicode word boundaries inside each cell; composes with literal or Regex mode.");
        if words.changed() && view.filter.whole_words { view.filter.fuzzy = false; }
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| { data.insert_temp(id.with("regex"), regex.rect); data.insert_temp(id.with("whole-words"), words.rect); });
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(id.with("fuzzy"), fuzzy.rect));
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = fuzzy;
        ui.checkbox(&mut view.filter.match_case, "Match case");
        ui.checkbox(&mut view.filter.smart_case, "Smart case")
            .on_hover_text("Uppercase in the query makes matching case-sensitive; Match case always takes precedence.");
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Search in");
        let scope = egui::ComboBox::from_id_salt(id.with("filter-column"))
            .selected_text(view.filter.column.map_or_else(|| "All columns".to_owned(), |column| format!("Column {}", column + 1)))
            .show_ui(ui, |ui| {
                let all = ui.selectable_value(&mut view.filter.column, None, "All columns");
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(id.with("filter-all"), all.rect));
                #[cfg(not(any(test, feature = "visual-qa")))]
                let _ = all;
                for column in 0..table.columns {
                    let label = format!("Column {}", column + 1);
                    let response = ui.selectable_value(&mut view.filter.column, Some(column), label);
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(id.with(("filter-column-option", column)), response.rect));
                    if view.first_heading {
                        response.on_hover_text(table.rows[0].get(column).map_or("", String::as_str));
                    }
                }
            });
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(id.with("filter-scope"), scope.response.rect));
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = scope;
        let exclude = ui.checkbox(&mut view.filter.exclude, "Exclude matches")
            .on_hover_text("Hide rows matching the nonempty query in the selected columns. Missing cells do not match. An empty query shows all rows.");
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(id.with("exclude-filter"), exclude.rect));
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = exclude;
    });
    ui.small(if view.numeric_sort {
        "Click a column: ascending, descending, original. Exact numeric order; non-numeric cells last. Only loaded rows."
    } else {
        "Click a column: ascending, descending, original. Natural, case-sensitive text order; only loaded rows."
    });
    let mut copy = None;
    let mut export_error = None;
    let mut rebuild = changed
        || before
            != (
                view.first_heading,
                view.sort,
                view.numeric_sort,
                view.filter.clone(),
            );
    egui::ScrollArea::horizontal()
        .id_salt(id.with("horizontal"))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized([40.0, 22.0], egui::Label::new("Row"));
                for column in 0..table.columns {
                    let label = if view.first_heading {
                        table.rows[0]
                            .get(column)
                            .map_or("", String::as_str)
                            .to_owned()
                    } else {
                        format!("Column {}", column + 1)
                    };
                    let arrow = match view.sort {
                        Some((current, false)) if current == column => " [asc]",
                        Some((current, true)) if current == column => " [desc]",
                        _ => "",
                    };
                    let button = ui
                        .add_sized(
                            [160.0, 22.0],
                            egui::Button::new(
                                egui::RichText::new(format!("{label}{arrow}")).strong(),
                            )
                            .truncate(),
                        )
                        .on_hover_text(format!(
                            "Column {}: {label}\nSort ascending / descending / original order",
                            column + 1
                        ));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(id.with(("column", column)), button.rect)
                    });
                    if button.clicked() {
                        choose_column(&mut view, column);
                        rebuild = true;
                    }
                }
            });
            if rebuild || !view.order_ready {
                let mut order = ordered_rows_with_policy(&table, view.first_heading, view.sort, view.numeric_sort);
                view.filter_error = view.filter.retain(&table, &mut order).err();
                view.order = Arc::new(order);
                view.order_ready = true;
            }
            if let Some(error) = &view.filter_error { ui.colored_label(ui.visuals().error_fg_color, error); }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.small(format!("Showing {} of {} data rows", view.order.len(), table.rows.len() - usize::from(view.first_heading)));
                let format = match delimiter { b'\t'=>"TSV", b';'=>"semicolon-separated", b'|'=>"pipe-separated", _=>"CSV" };
                let response = ui.add_enabled(!view.order.is_empty(), egui::Button::new(format!("Copy visible {format} ({})", view.order.len())))
                    .on_hover_text("Copy shown records in the displayed order, with headings when enabled. Only loaded preview rows; quotes, separators and line breaks are escaped. Cell values are unchanged.");
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(id.with("copy-records"), response.rect));
                if response.clicked() {
                    match export_rows(&table, &view, delimiter) {
                        Ok(text) => copy = Some(text),
                        Err(error) => export_error = Some(error),
                    }
                }
            });
            if let Some(error) = &export_error { ui.colored_label(ui.visuals().error_fg_color, error); }
            if view.order.is_empty() {
                ui.label("No matching rows.");
            }
            let mut scroll = egui::ScrollArea::vertical().id_salt(id.with("scroll"));
            if rebuild {
                scroll = scroll.scroll_offset(egui::Vec2::ZERO);
            }
            scroll.show_rows(ui, 22.0, view.order.len(), |ui, range| {
                egui::Grid::new(id.with("grid"))
                    .striped(true)
                    .min_col_width(0.0)
                    .show(ui, |ui| {
                        for position in range {
                            let row = view.order[position];
                            ui.add_sized(
                                [40.0, 22.0],
                                egui::Label::new(egui::RichText::new((row + 1).to_string()).weak()),
                            );
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
        });
    ui.ctx()
        .data_mut(|data| data.insert_temp(id.with("view"), view));
    copy
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delimiter_detection_is_quote_aware_unique_and_sample_bounded() {
        for delimiter in *b",\t;|" {
            let d = char::from(delimiter);
            let source = format!("\u{feff}Left{d}Right\none{d}\"a,b;|two\nline\"\nthree{d}four\n");
            assert_eq!(detect_delimiter(&source), Some(delimiter));
        }
        for source in [
            "",
            "a,b",
            "one\ntwo\n",
            "a,b;c\n1,2;3\n",
            "a,b\n1,2,3\n",
            "a:b\n1:2\n",
        ] {
            assert_eq!(detect_delimiter(source), None, "{source}");
        }
        let wide = format!("{}\n{}\n", vec!["x"; 65].join(","), vec!["1"; 65].join(","));
        assert_eq!(detect_delimiter(&wide), None);
        let sampled = format!("{}a;b;c\n", "a;b\n".repeat(32));
        assert_eq!(detect_delimiter(&sampled), Some(b';'));
        assert_eq!(
            detect_delimiter(&format!("z{}", "ё".repeat(MAX_BYTES / 2 + 1))),
            None
        );
    }

    #[test]
    fn ambiguous_detection_keeps_current_table_filter_sort_and_copy_until_manual_choice() {
        let ctx = egui::Context::default();
        let content: Arc<str> = Arc::from("A,B;C\n1,2;3\n4,5;6\n");
        let id = egui::Id::new(("table_preview", "right"));
        let frame = |content: &Arc<str>, events| {
            let mut copied = None;
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| copied = render(ui, content, b',', "right"),
            ));
            copied
        };
        let click = |key| {
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(key))
                .unwrap()
                .center();
            let mut copied = None;
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                if let Some(text) = frame(&content, events) {
                    copied = Some(text);
                }
            }
            copied
        };
        let choose_comma = || {
            click(id.with("delimiter"));
            click(id.with(("delimiter-option", "Comma")));
        };
        frame(&content, vec![]);
        choose_comma();
        click(id.with("heading"));
        click(id.with("numeric-sort"));
        click(id.with(("column", 0usize)));
        click(id.with("filter"));
        frame(&content, vec![egui::Event::Text("1".into())]);
        let before = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        let parsed = ctx.data(|data| data.get_temp::<CachedTable>(id)).unwrap();
        click(id.with("detect-delimiter"));
        let choice = ctx
            .data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
            .unwrap();
        assert!(choice.detection_error && choice.selected == Some(b','));
        let after = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert_eq!(*after.order, [1]);
        assert!(
            after.first_heading
                && after.numeric_sort
                && after.sort == before.sort
                && after.filter == before.filter
        );
        assert!(Arc::ptr_eq(&before.order, &after.order));
        assert!(Arc::ptr_eq(
            &parsed.table,
            &ctx.data(|data| data.get_temp::<CachedTable>(id))
                .unwrap()
                .table
        ));
        assert_eq!(
            click(id.with("copy-records")).as_deref(),
            Some("A,B;C\n1,2;3\n")
        );
        choose_comma();
        assert!(
            !ctx.data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
                .unwrap()
                .detection_error
        );
        click(id.with("detect-delimiter"));
        let refreshed: Arc<str> = Arc::from(content.as_ref());
        frame(&refreshed, vec![]);
        let reset = ctx
            .data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
            .unwrap();
        assert!(!reset.detection_error && reset.selected.is_none());
    }

    #[test]
    fn delimiter_menu_reparses_quotes_exports_actual_dialect_and_resets_source_owned_state() {
        let ctx = egui::Context::default();
        let content: Arc<str> =
            Arc::from("Name;Value;Note\nb;2;\"one;two|three\"\na;-10;\"line\nbreak\"\n");
        let original = content.to_string();
        let id = egui::Id::new(("table_preview", "right"));
        let frame = |content: &Arc<str>, side: &str, default: u8, events| {
            let mut copied = None;
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| copied = render(ui, content, default, side),
            ));
            copied
        };
        let click = |content: &Arc<str>, key| {
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(key))
                .unwrap()
                .center();
            let mut copied = None;
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                if let Some(text) = frame(content, "right", b',', events) {
                    copied = Some(text);
                }
            }
            copied
        };
        let choose = |content: &Arc<str>, label| {
            let _ = click(content, id.with("delimiter"));
            let _ = click(content, id.with(("delimiter-option", label)));
        };
        frame(&content, "right", b',', vec![]);
        assert_eq!(
            ctx.data(|data| data.get_temp::<CachedTable>(id))
                .unwrap()
                .table
                .columns,
            1
        );
        click(&content, id.with("detect-delimiter"));
        assert_eq!(
            ctx.data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
                .unwrap()
                .selected,
            Some(b';')
        );
        let cached = ctx.data(|data| data.get_temp::<CachedTable>(id)).unwrap();
        assert_eq!(cached.delimiter, b';');
        assert_eq!(cached.table.rows.len(), 3);
        assert_eq!(cached.table.rows[1], ["b", "2", "one;two|three"]);
        assert_eq!(cached.table.rows[2], ["a", "-10", "line\nbreak"]);
        click(&content, id.with("heading"));
        click(&content, id.with("numeric-sort"));
        click(&content, id.with(("column", 1usize)));
        click(&content, id.with("filter"));
        frame(
            &content,
            "right",
            b',',
            vec![egui::Event::Text("one".into())],
        );
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![1])));
        assert_eq!(
            click(&content, id.with("copy-records")).as_deref(),
            Some("Name;Value;Note\nb;2;\"one;two|three\"\n")
        );
        choose(&content, "Pipe");
        let reset = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert!(
            !reset.first_heading
                && !reset.numeric_sort
                && reset.sort.is_none()
                && reset.filter.query.is_empty()
        );
        assert_eq!(content.as_ref(), original);
        let next: Arc<str> = Arc::from("Name|Value\nx|-2\n");
        frame(&next, "right", b',', vec![]);
        assert_eq!(
            ctx.data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
                .unwrap()
                .selected,
            None
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<CachedTable>(id))
                .unwrap()
                .delimiter,
            b','
        );
        choose(&next, "Pipe");
        assert_eq!(
            ctx.data(|data| data.get_temp::<CachedTable>(id))
                .unwrap()
                .table
                .rows[1],
            ["x", "-2"]
        );
        frame(&next, "left", b'\t', vec![]);
        let left = egui::Id::new(("table_preview", "left"));
        assert_eq!(
            ctx.data(|data| data.get_temp::<DialectChoice>(left.with("dialect")))
                .unwrap()
                .selected,
            None
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<CachedTable>(left))
                .unwrap()
                .delimiter,
            b'\t'
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<DialectChoice>(id.with("dialect")))
                .unwrap()
                .selected,
            Some(b'|')
        );
    }

    #[test]
    fn numeric_sort_preserves_precision_signed_values_ties_and_invalid_tail() {
        let values = [
            "9007199254740993",
            "9007199254740992",
            "-10",
            "-2",
            "0",
            "-0.00",
            "1.20",
            "1.2e0",
            ".001",
            "1e-1000000",
            "-1e-1000000",
            "1e1000000",
            "bad",
            "",
            "NaN",
            "Infinity",
        ];
        let mut input = "N,Tag\n".to_owned();
        for value in values {
            input.push_str(&format!("{value},keep\n"));
        }
        input.push_str("missing\n");
        let table = parse(&input, b',');
        assert_eq!(
            ordered_rows_with_policy(&table, true, Some((0, false)), true),
            [3, 4, 11, 5, 6, 10, 9, 7, 8, 2, 1, 12, 13, 14, 15, 16, 17]
        );
        assert_eq!(
            ordered_rows_with_policy(&table, true, Some((0, true)), true),
            [12, 1, 2, 7, 8, 9, 10, 5, 6, 11, 4, 3, 13, 14, 15, 16, 17]
        );
        let ragged = parse("Name,Value\na,2\nb\nc,-1\nd,\n", b',');
        assert_eq!(
            ordered_rows_with_policy(&ragged, true, Some((1, false)), true),
            [3, 1, 2, 4]
        );
        assert_eq!(
            ordered_rows_with_policy(&ragged, true, Some((1, true)), true),
            [1, 3, 2, 4]
        );
        assert_eq!(
            ordered_rows_with_policy(&table, true, None, true),
            (1..table.rows.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn decimal_order_agrees_with_independent_small_rational_oracle() {
        let mut numbers = Vec::new();
        for value in -35_i128..=35 {
            for scale in 0..=3_u32 {
                numbers.push((
                    DecimalKey::parse(&format!("{value}e-{scale}")).unwrap(),
                    value,
                    10_i128.pow(scale),
                ));
            }
        }
        for (a, numerator_a, denominator_a) in &numbers {
            for (b, numerator_b, denominator_b) in &numbers {
                assert_eq!(
                    a.compare(b),
                    (numerator_a * denominator_b).cmp(&(numerator_b * denominator_a))
                );
            }
        }
        for text in [
            "",
            ".",
            "NaN",
            "inf",
            "1,000",
            "0x10",
            "1e",
            "1e1000001",
            "1e-1000001",
            "1.2.3",
            "--1",
            "١",
        ] {
            assert!(DecimalKey::parse(text).is_none(), "{text}");
        }
        assert!(DecimalKey::parse(&"1".repeat(4096)).is_some());
        assert!(DecimalKey::parse(&"1".repeat(4097)).is_none());
        assert_eq!(
            DecimalKey::parse("  +001.200E+2 ").unwrap(),
            DecimalKey::parse("120").unwrap()
        );
    }

    #[test]
    fn numeric_toggle_rebuilds_ui_order_and_filtered_exports_without_changing_cells() {
        let ctx = egui::Context::default();
        let content: Arc<str> =
            Arc::from("N,Tag\n-2,keep\n-10,keep\n2.1,skip\n2.01,keep\n2.100,keep\nbad,keep\n");
        let original = content.to_string();
        let id = egui::Id::new(("table_preview", "right"));
        let frame = |content: &Arc<str>, events| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let _ = render(ui, content, b',', "right");
                },
            ));
        };
        let click = |key| {
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(key))
                .unwrap()
                .center();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(&content, events);
            }
        };
        frame(&content, vec![]);
        click(id.with("heading"));
        click(id.with(("column", 0usize)));
        click(id.with("numeric-sort"));
        assert_eq!(
            shown_order(&ctx, "right"),
            Some((true, vec![2, 1, 4, 3, 5, 6]))
        );
        click(id.with(("column", 0usize)));
        assert_eq!(
            shown_order(&ctx, "right"),
            Some((true, vec![3, 5, 4, 1, 2, 6]))
        );
        click(id.with("filter"));
        frame(&content, vec![egui::Event::Text("keep".into())]);
        let view = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert_eq!(*view.order, [5, 4, 1, 2, 6]);
        assert_eq!(
            export_rows(&parse(&content, b','), &view, b',').unwrap(),
            "N,Tag\n2.100,keep\n2.01,keep\n-2,keep\n-10,keep\nbad,keep\n"
        );
        assert_eq!(content.as_ref(), original);
        let refreshed: Arc<str> = Arc::from(original);
        frame(&refreshed, vec![]);
        let fresh = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert!(!fresh.numeric_sort && fresh.sort.is_none() && !fresh.first_heading);
    }

    #[test]
    fn heading_and_natural_sort_preserve_record_identity_and_equal_key_order() {
        let table = parse(
            "Name,Value\nfile10,2\nfile2,10\nfile2,1\nёж,3\nlast\n",
            b',',
        );
        assert_eq!(ordered_rows(&table, false, None), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(
            ordered_rows(&table, true, Some((0, false))),
            vec![2, 3, 1, 5, 4]
        );
        assert_eq!(
            ordered_rows(&table, true, Some((0, true))),
            vec![4, 5, 1, 2, 3]
        );
        assert_eq!(
            ordered_rows(&table, true, Some((1, false))),
            vec![5, 3, 1, 4, 2]
        );
        assert_eq!(
            ordered_rows(&table, true, Some((64, false))),
            vec![1, 2, 3, 4, 5]
        );
        let mut view = View::default();
        choose_column(&mut view, 0);
        assert_eq!(view.sort, Some((0, false)));
        choose_column(&mut view, 0);
        assert_eq!(view.sort, Some((0, true)));
        choose_column(&mut view, 0);
        assert_eq!(view.sort, None);
        choose_column(&mut view, 1);
        assert_eq!(view.sort, Some((1, false)));
        assert!(ordered_rows(&parse("heading", b','), true, Some((0, false))).is_empty());
        assert!(ordered_rows(&parse("", b','), true, None).is_empty());
    }

    #[test]
    fn pointer_heading_sort_and_reload_keep_panes_and_source_independent() {
        let ctx = egui::Context::default();
        let content: Arc<str> = Arc::from("Name,Count\nfile10,2\nfile2,10\nfile2,1\nёж,3\n");
        let original = content.to_string();
        let frame = |content: &Arc<str>, side: &str, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 400.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let _ = render(ui, content, b',', side);
            });
            output.textures_delta.clear();
        };
        frame(&content, "right", vec![]);
        assert_eq!(
            shown_order(&ctx, "right"),
            Some((false, vec![0, 1, 2, 3, 4]))
        );
        let id = egui::Id::new(("table_preview", "right"));
        for (key, expected) in [
            (id.with("heading"), (true, vec![1, 2, 3, 4])),
            (id.with(("column", 0usize)), (true, vec![2, 3, 1, 4])),
            (id.with(("column", 0usize)), (true, vec![4, 1, 2, 3])),
            (id.with(("column", 0usize)), (true, vec![1, 2, 3, 4])),
        ] {
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(key))
                .unwrap()
                .center();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(&content, "right", events);
            }
            assert_eq!(shown_order(&ctx, "right"), Some(expected));
        }
        frame(&content, "left", vec![]);
        assert_eq!(
            shown_order(&ctx, "left"),
            Some((false, vec![0, 1, 2, 3, 4]))
        );
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![1, 2, 3, 4])));
        let reloaded: Arc<str> = Arc::from(content.as_ref());
        frame(&reloaded, "right", vec![]);
        assert_eq!(
            shown_order(&ctx, "right"),
            Some((false, vec![0, 1, 2, 3, 4]))
        );
        frame(&Arc::from(""), "right", vec![]);
        assert!(shown_order(&ctx, "right").is_none());
        assert_eq!(content.as_ref(), original);
    }

    #[test]
    fn row_filter_composes_with_heading_sort_unicode_and_literal_cell_boundaries() {
        let table = parse(
            "Name,Note\nfile10,ёж\nfile2,one two\nfile2,ONE\nФАЙЛ,other\nleft,right\n",
            b',',
        );
        let filtered = |filter: RowFilter, heading| {
            let mut rows = ordered_rows(&table, heading, Some((0, false)));
            let _ = filter.retain(&table, &mut rows);
            rows
        };
        let query = |text: &str| RowFilter {
            query: text.into(),
            ..Default::default()
        };
        assert_eq!(filtered(query("file2"), true), vec![2, 3]);
        assert!(filtered(query("fl2"), true).is_empty());
        assert_eq!(
            filtered(
                RowFilter {
                    fuzzy: true,
                    ..query("fl2")
                },
                true
            ),
            vec![2, 3]
        );
        assert_eq!(filtered(query(" ЁЖ "), true), vec![1]);
        assert_eq!(filtered(query("файл"), true), vec![4]);
        assert!(
            filtered(
                RowFilter {
                    match_case: true,
                    ..query("файл")
                },
                true
            )
            .is_empty()
        );
        assert!(
            filtered(
                RowFilter {
                    smart_case: true,
                    ..query("Файл")
                },
                true
            )
            .is_empty()
        );
        assert_eq!(
            filtered(
                RowFilter {
                    smart_case: true,
                    ..query("ФАЙЛ")
                },
                true
            ),
            vec![4]
        );
        assert_eq!(filtered(query("one two"), true), vec![2]);
        assert!(filtered(query("leftright"), true).is_empty());
        assert!(
            filtered(
                RowFilter {
                    fuzzy: true,
                    ..query("leftright")
                },
                true
            )
            .is_empty()
        );
        assert!(filtered(query("Name"), true).is_empty());
        assert_eq!(filtered(query("Name"), false), vec![0]);
        assert_eq!(filtered(query("   "), true), vec![2, 3, 1, 5, 4]);
        assert!(filtered(query(&"ё".repeat(257)), true).is_empty());
        assert_eq!(table.rows[0], ["Name", "Note"]);
    }

    #[test]
    fn regex_word_filters_bound_errors_and_preserve_cell_boundaries() {
        let table = parse(
            "Name,Note\ncat,ЁЖ\nconcatenate,ёжик\ncat_dog,other\nleft,right\n",
            b',',
        );
        let run = |filter: RowFilter| {
            let mut rows = ordered_rows(&table, true, None);
            let result = filter.retain(&table, &mut rows);
            (rows, result)
        };
        let words = RowFilter {
            query: "cat".into(),
            whole_words: true,
            ..Default::default()
        };
        assert_eq!(run(words.clone()).0, [1]);
        assert_eq!(
            run(RowFilter {
                exclude: true,
                ..words
            })
            .0,
            [2, 3, 4]
        );
        let regex = RowFilter {
            query: "^ёж$".into(),
            regex: true,
            column: Some(1),
            ..Default::default()
        };
        assert_eq!(run(regex.clone()).0, [1]);
        assert!(
            run(RowFilter {
                match_case: true,
                ..regex
            })
            .0
            .is_empty()
        );
        assert!(
            run(RowFilter {
                query: "left.*right".into(),
                regex: true,
                ..Default::default()
            })
            .0
            .is_empty()
        );
        assert_eq!(
            run(RowFilter {
                query: "(?x) cat # comment".into(),
                regex: true,
                whole_words: true,
                ..Default::default()
            })
            .0,
            [1]
        );
        for query in ["[".to_owned(), "(".repeat(40), "ё".repeat(257)] {
            for exclude in [false, true] {
                let (rows, result) = run(RowFilter {
                    query: query.clone(),
                    regex: true,
                    exclude,
                    ..Default::default()
                });
                assert!(result.is_err() && rows.is_empty());
            }
        }
    }

    #[test]
    fn scoped_inverse_filter_preserves_record_order_and_export_ownership() {
        let table = parse("Name,Note\nfile10,keep\nkeep,FILE2\nfile2\nother,\n", b',');
        let mut view = View {
            first_heading: true,
            sort: Some((0, false)),
            filter: RowFilter {
                query: "file2".into(),
                column: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        let update = |view: &mut View| {
            let mut order =
                ordered_rows_with_policy(&table, view.first_heading, view.sort, view.numeric_sort);
            view.filter_error = view.filter.retain(&table, &mut order).err();
            view.order = Arc::new(order);
            view.order_ready = true;
        };
        update(&mut view);
        assert_eq!(*view.order, [2]);
        view.filter.exclude = true;
        update(&mut view);
        assert_eq!(*view.order, [3, 1, 4]);
        assert_eq!(
            export_rows(&table, &view, b',').unwrap(),
            "Name,Note\nfile2\nfile10,keep\nother,\n"
        );
        view.filter.column = None;
        update(&mut view);
        assert_eq!(*view.order, [1, 4]);
        view.filter.query = "fl2".into();
        view.filter.fuzzy = true;
        view.filter.column = Some(1);
        update(&mut view);
        assert_eq!(*view.order, [3, 1, 4]);
        view.filter.match_case = true;
        update(&mut view);
        assert_eq!(*view.order, [3, 1, 2, 4]);
        view.filter.query = "   ".into();
        update(&mut view);
        assert_eq!(*view.order, [3, 1, 2, 4]);
        view.filter.query = "ё".repeat(257);
        update(&mut view);
        assert!(view.order.is_empty());
        view.filter.query = "file2".into();
        view.filter.column = Some(2);
        update(&mut view);
        assert!(view.order.is_empty());
        assert_eq!(table.rows[2], ["keep", "FILE2"]);
    }

    #[test]
    fn pointer_row_filter_fuzzy_clear_and_cached_empty_results_preserve_sort() {
        let ctx = egui::Context::default();
        let content: Arc<str> = Arc::from("Name,Count\nfile10,2\nfile2,10\nfile2,1\nёж,3\n");
        let frame = |content: &Arc<str>, side, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 550.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let _ = render(ui, content, b',', side);
                },
            );
            output.textures_delta.clear();
        };
        let id = egui::Id::new(("table_preview", "right"));
        let click = |key| {
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(key))
                .unwrap()
                .center();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(&content, "right", events);
            }
        };
        frame(&content, "right", vec![]);
        click(id.with("heading"));
        click(id.with(("column", 0usize)));
        click(id.with("filter"));
        frame(&content, "right", vec![egui::Event::Text("fl2".into())]);
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![])));
        let before = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        frame(&content, "right", vec![]);
        let after = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert!(Arc::ptr_eq(&before.order, &after.order));
        click(id.with("fuzzy"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![2, 3])));
        click(id.with("regex"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![])));
        assert!(
            !ctx.data(|data| data.get_temp::<View>(id.with("view")))
                .unwrap()
                .filter
                .fuzzy
        );
        click(id.with("fuzzy"));
        click(id.with("whole-words"));
        let mode = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert!(!mode.filter.fuzzy && !mode.filter.regex && mode.filter.whole_words);
        click(id.with("fuzzy"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![2, 3])));

        click(id.with("filter-scope"));
        click(id.with(("filter-column-option", 1usize)));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![])));
        click(id.with("exclude-filter"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![2, 3, 1, 4])));
        click(id.with("filter-scope"));
        click(id.with("filter-all"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![1, 4])));
        click(id.with("exclude-filter"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![2, 3])));
        click(id.with("clear-filter"));
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![2, 3, 1, 4])));
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(id.with("filter-input"))
        );
        frame(&content, "right", vec![egui::Event::Text("fl2".into())]);
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![2, 3])));
        frame(&content, "left", vec![]);
        assert_eq!(
            shown_order(&ctx, "left"),
            Some((false, vec![0, 1, 2, 3, 4]))
        );
        let reloaded: Arc<str> = Arc::from(content.as_ref());
        frame(&reloaded, "right", vec![]);
        let reset = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        assert!(reset.filter.query.is_empty() && !reset.filter.fuzzy && !reset.first_heading);
        assert!(reset.sort.is_none());
    }

    #[test]
    fn visible_record_export_round_trips_csv_tsv_headings_ragged_and_literal_values() {
        let table = parse(
            "Name,Note,End\nfile10,hidden\nfile2,\"comma, quote \"\" and\nline\",\nёж,=SUM(A1:A2)\nlast\n",
            b',',
        );
        let view = View {
            first_heading: true,
            order_ready: true,
            order: Arc::new(vec![3, 2, 4]),
            ..Default::default()
        };
        for delimiter in *b",\t;|" {
            let exported = export_rows(&table, &view, delimiter).unwrap();
            let reparsed = parse(&exported, delimiter);
            assert_eq!(
                reparsed.rows,
                [
                    table.rows[0].clone(),
                    table.rows[3].clone(),
                    table.rows[2].clone(),
                    table.rows[4].clone()
                ]
            );
            assert!(!exported.contains("hidden"));
            assert!(exported.contains("=SUM(A1:A2)"));
            assert!(exported.ends_with("last\n"));
            assert!(exported.contains("and\nline"));
        }
        let headerless = View {
            order_ready: true,
            order: Arc::new(vec![0, 2]),
            ..Default::default()
        };
        assert_eq!(
            parse(&export_rows(&table, &headerless, b',').unwrap(), b',').rows,
            [table.rows[0].clone(), table.rows[2].clone()]
        );
        assert!(export_rows(&table, &View::default(), b',').is_err());
        assert!(
            export_rows(
                &table,
                &View {
                    order_ready: true,
                    ..Default::default()
                },
                b','
            )
            .is_err()
        );
        assert!(export_rows(&table, &view, b':').is_err());
        let invalid = View {
            first_heading: true,
            order_ready: true,
            order: Arc::new(vec![0]),
            ..Default::default()
        };
        assert!(export_rows(&table, &invalid, b',').is_err());
        let invalid = View {
            order_ready: true,
            order: Arc::new(vec![100]),
            ..Default::default()
        };
        assert!(export_rows(&table, &invalid, b',').is_err());
        let oversized = Table {
            rows: vec![vec!["\"".repeat(1024 * 1024)]],
            columns: 1,
            limited: false,
            error: None,
        };
        assert!(
            export_rows(
                &oversized,
                &View {
                    order_ready: true,
                    order: Arc::new(vec![0]),
                    ..Default::default()
                },
                b','
            )
            .unwrap_err()
            .contains("1 MiB")
        );
    }

    #[test]
    fn pointer_record_copy_requests_exact_filtered_records_once_and_blocks_empty_views() {
        let ctx = egui::Context::default();
        let content: Arc<str> = Arc::from("Name,Count\nfile10,2\nfile2,10\nfile2,1\nёж,3\n");
        let requests = std::cell::RefCell::new(Vec::new());
        let frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 550.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(text) = render(ui, &content, b',', "right") {
                        requests.borrow_mut().push(text);
                    }
                },
            );
            output.textures_delta.clear();
        };
        frame(vec![]);
        let id = egui::Id::new(("table_preview", "right"));
        ctx.data_mut(|data| {
            data.insert_temp(
                id.with("view"),
                View {
                    first_heading: true,
                    sort: Some((0, true)),
                    filter: RowFilter {
                        query: "file2".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
        });
        frame(vec![]);
        let click = || {
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(id.with("copy-records")))
                .unwrap()
                .center();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(events);
            }
        };
        click();
        assert_eq!(*requests.borrow(), ["Name,Count\nfile2,10\nfile2,1\n"]);
        let mut empty = ctx
            .data(|data| data.get_temp::<View>(id.with("view")))
            .unwrap();
        empty.filter.query = "absent".into();
        empty.order_ready = false;
        ctx.data_mut(|data| data.insert_temp(id.with("view"), empty));
        frame(vec![]);
        click();
        assert_eq!(requests.borrow().len(), 1);
        assert_eq!(shown_order(&ctx, "right"), Some((true, vec![])));
        assert_eq!(
            content.as_ref(),
            "Name,Count\nfile10,2\nfile2,10\nfile2,1\nёж,3\n"
        );
    }

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
