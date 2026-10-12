# Security

Treat model-generated commands and code as untrusted. LCA can read,
change, and execute files with the permissions of the account that
started it. Extensions run sandboxed with only the capabilities
their manifest declares — but the model punching the keys is still
the model, and prompt injection (hostile instructions hiding in
files, comments, command output, or web pages) steers it without
touching any boundary. No setting below makes untrusted content
safe; they limit what a wrong or hostile action can reach.

## The permission model in one minute

Deny by default. An extension declares what it wants
(`capabilities.fs` scopes, `net` host patterns, tools); you approve
it at install on a consent screen written in the catalog's words,
and anything undeclared is refused and recorded. At runtime:

- `/trust` decides whether the project directory's own settings and
  resources load (`ask` / `always` / `never`). `-a`/`--approve`
  trusts for one run, `--no-approve` untrusts for one run.
  Untrusted means project files cannot enable extensions or loosen
  permissions — and a project file that tries to grant its own
  trust is refused at load.
- `/grants` and `/permissions` show what is allowed; `allow`/`deny`
  rules carve the standing policy per scope and host.
- `/yolo` drops the per-action prompts for a run and says so in the
  footer (`[YOLO]`); it never widens what is granted, only how
  often you are asked.

A first request to an undeclared host offers a one-time ad hoc
grant (`allow` persists it for the project); headless runs exit 4
naming the host instead (see [CLI integration](cli-integration.md)).

## Credentials

API keys and tokens live in the extension credential store — the
masked `/login` field writes them there, and the provider reads
them back at request time. They never go in configuration files
and never in the session log. If a key appears in a transcript
(pasted by you, echoed by a tool), rotate it: the log is
append-only and redaction is not retroactive.

## Sessions and exports

The session log holds your prompts, tool arguments, command output,
and file contents. `/export` strips audit-only records by default
(permission answers, extension internals, usage accounting), but a
default export still contains the conversation — review it before
sharing, and treat a debug log the same way.

## Isolation

Strongest practical option first: run the agent itself inside a
container, VM, or sandbox and expose only the files and services
the task needs. Next: a dedicated OS user, which narrows what the
agent's permissions can reach but shares the machine. Last, and
still worth doing: short-lived, narrowly scoped credentials,
version control before big changes, and reviewing diffs before they
leave the machine.

## Report a security issue

Use GitHub's private reporting for this repository (see
`SECURITY.md` at the repo root). Do not open a public issue for a
security-sensitive report. Expected local-agent behavior, prompt
injection from untrusted content, and behavior from extensions or
skills you installed are generally outside the boundary unless the
report shows a privilege-boundary bypass — access the local user
did not already have. The developer-facing threat model
([threat-model](threat-model.md)) names the exact surfaces and
their guards.
