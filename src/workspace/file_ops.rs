//! File operations extracted for DRY (copy, move, delete high level).
//! SRP: group ops logic here, used from execute and handlers.
//! Inspired by Total Commander operations.

use crate::workspace::Workspace;
use crate::command::Command;

/// Stub handler for file ops commands.
pub fn handle_file_ops(ws: &mut Workspace, cmd: Command) -> bool {
    match cmd {
        Command::RequestCopy => {
            ws.request_copy();
            true
        }
        Command::RequestMove => {
            ws.request_move();
            true
        }
        Command::RequestDelete => {
            ws.request_delete();
            true
        }
        _ => false,
    }
}
