# Pending workflows

`ci.yml` is the complete continuous-integration workflow: three-OS test
matrix (NFR-12), fmt/clippy/doc gates, cargo-nextest, cargo-deny (NFR-16),
the requirements-traceability check (NFR-30), and the size/startup gate
(NFR-7). It lives here because the repository's current git credential is a
fine-grained personal access token without the **Workflows** permission, so
GitHub rejects any push or API call that touches `.github/workflows/`
(`refusing to allow a Personal Access Token to create or update workflow
.github/workflows/ci.yml without workflow scope`).

To activate it, once the credential carries that permission:

```sh
mkdir -p .github/workflows
git mv ci/pending-workflows/ci.yml .github/workflows/ci.yml
git commit -m "ci: activate the three-OS pipeline"
git push origin main
```

The gate scripts the workflow calls are live now and runnable locally:
`bash scripts/traceability.sh`, `bash scripts/perf-gate.sh`.
