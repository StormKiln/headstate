# Compare repository configuration

In **Settings → Claude Code**, enable the integration and open **Compare
repository configurations**. Select a repository and either another repository
or **User configuration**. Results are cached for that pair; use **Refresh
comparison** after editing files. A failed refresh leaves the previous result
visible with an error. Changing the pair never displays the previous pair's result.

The comparison groups entries as Same, Different, Only on the left, Only on the
right, and Cannot compare. Unreadable files or incomplete directory scans cannot
establish that an entry is absent. Each entry shows its source and both values.
The view never modifies configuration.

Settings compare the existing resolver's tracked keys: permissions, model, env,
and outputStyle. Hooks and skill/agent/command definitions compare explicit user,
project, and local scopes where applicable; they do not claim a merged winner.
CLAUDE.md files compare text at checkout-relative paths, with CRLF normalized.
Imports are not expanded. Plugin, managed, and command-line configuration is not
included. Nested repositories, worktrees, and dependency/build directories are
excluded. Only repositories authorized by the configured repository scan can be
selected; arbitrary directories are refused by the native command.

Each side allows 4096 directory entries, 2 MiB of file reads, and 256 KiB per file.
Exceeding a bound produces an unknown/partial result. Symlinks are not followed.
Equality uses the complete text read within those bounds; previews are limited to
8 KiB. Phone previews mask recognizable secrets before truncation. This masking
is best effort, and two identical masked previews can still represent different
original values.

The reusable `/harness/config-comparison.html` preview contains synthetic data
only and can be used for screenshots. It does not access real repositories.
