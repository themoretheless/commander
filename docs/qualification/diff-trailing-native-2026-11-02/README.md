# Native trailing-space policy integration

The new trailing-only checkbox is placed on its own wrapped row after the established comparison/intraline settings, retaining the old controls' layout. DiffOptions now extends its existing 17-action tab/space/case/endings/reload/swap workflow with disabling Ignore all spaces and clicking Ignore trailing spaces. The native driver uses pointer events and actual retained-source recomparison, not direct option assignment. Expected stage counts retain the preceding comparison assertions; final state requires trailing_spaces true, all_spaces false, two real change hunks, and exact original source ownership/text after Swap.

The existing fixture has no trailing-only difference; this native scenario proves the new checkbox integration and composition, while dedicated textdiff tests prove removal of trailing-only differences and preservation of significant leading/interior spacing/endings. It does not establish native trailing-only fixture behavior. No scene deadline or expected effect policy is loosened. Compile/test work finishes before launch for this run.

Prior failed DiffOptions attempts are preserved in diff-trailing-spaces-2026-11-02; they are not counted as successful evidence. Fresh output and launch hashes below belong only to this run. Native OS pasteboard is irrelevant: this workflow expects zero native effects.
