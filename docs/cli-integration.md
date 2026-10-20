# CLI integration cookbook

`lca -p` runs headless turns: no interface, final reply on stdout.
This page is recipes; the contract (flags, event stream, RPC loop,
stability promises) is [headless](headless.md).

## One-shot questions

```sh
lca -p "what does this crate export?"          # reply on stdout
git diff | lca -p "review this change"         # piped stdin becomes the first prompt
lca -p "summarize" --model "zen/*"             # fuzzy model pattern for the run
lca --list-models sonnet                       # id  provider  context lines, filtered
```

A redirected stdin or stdout without `--mode` also means print
mode, so pipelines compose without flags.

## Keep a session across runs

```sh
lca -p "start the refactor"                    # fresh session
lca -c -p "continue it"                        # append to this project's latest session
lca -r <id> -p "a follow-up"                   # append to that session
lca --no-session -p "throwaway question"       # volatile: temp dir, dropped at exit
lca resume                                     # list this project's sessions
```

## Structured output

```sh
lca -p "..." --mode json | jq -r 'select(.type=="text") | .content'
lca -p "..." --mode json | jq -c 'select(.type=="tool-call") | {name, arguments}'
lca -p "..." --mode json | jq -s '[.[] | select(.type=="usage") | .cost] | add'
```

Stdout carries records only; diagnostics go to stderr, so `2>/dev/null`
is safe. The envelope may gain fields; fields are never removed or
retyped within a major version.

## Exit codes (stable within a major)

| Code | Meaning | Reach for |
|---|---|---|
| 0 | Ok | |
| 1 | Internal error | `--verbose`, `~/.lca/logs/lca.log` |
| 2 | Usage error (bad flags, bad paths, empty print run) | Fix the invocation |
| 3 | Provider failure (auth, endpoint, model) | `/login`, `OPENAI_API_KEY`, `--allow-host` |
| 4 | Permission (undeclared host, refused grant) | Run once interactively and allow, or `--allow-host <host>` for this run only |
| 5 | Aborted (cancelled turn) | Retry, or narrower scope |
| 6 | Session (nothing to continue, bad session id) | `lca resume` to see what exists |

```sh
lca -p "deploy check" || case $? in
  4) echo "grant the host first: lca, allow once, retry" ;;
  3) echo "provider down or key wrong" ;;
  *) echo "see lca.log" ;;
esac
```

## Custom endpoints in scripts

```sh
export OPENAI_BASE_URL="https://models.example.com/v1"
export OPENAI_API_KEY="$(cat ~/.secrets/models-key)"   # never paste a key through the shell
lca -p "hi" --allow-host models.example.com            # this run only, never persisted
```

`export KEY=$(cat file)` keeps the secret out of scrollback and
history. `--allow-host` repeats for several hosts and is validated
at parse time (a bad pattern is exit 2).

## Scope the run down

```sh
lca -p "read-only audit" --no-tools                   # model answers from context alone
lca -p "lint" -t 'shell,read'                         # allowlist; `+`/`-` deltas and `*` globs
lca -p "try it" -e ./my-ext.wasm                     # one-run extension, same consent as installed
lca -p "quiet" --no-extensions --no-skills            # providers still resolve
lca -p "no mcp" --no-mcp                              # no server spawns, no endpoint contacted
lca -a -p "trusted run"                               # project files load for this run, nothing persists
```

## Long-lived control (`--mode rpc`)

```sh
lca --mode rpc --no-session <<'EOF'
{"type":"prompt","id":1,"message":"start the migration"}
{"type":"shutdown"}
EOF
```

One JSON object per line on stdin, responses plus the event stream
on stdout. A `prompt` response means accepted or queued, not done —
consume through `agent_settled` when completion matters. Closing
stdin persists the session and exits 0.
