# AI Agent Rules

## Blacklisted Operations
The following destructive operations/commands MUST NOT be executed:
- `git reset`
- `rm` (unless explicitly deleting temporary/scratch files)
- `git filter-repo`
- `git clean`

## Blacklisted Dependencies
- `dotenv` is blacklisted across all repositories. Do not install or use it.

