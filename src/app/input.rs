//! Input handling stub.
//! New idea: separate key handling, typeahead, etc from update.rs.
//! For SRP, like in VSCode keybinding system.

pub fn handle_type_ahead(buffer: &str) -> bool {
    // TODO: move type_ahead logic here.
    !buffer.is_empty()
}
