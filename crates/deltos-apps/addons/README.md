# Example add-on manifests (drafts)

Worked examples of the add-on format (`docs/base-addons-admin.md` §5), used by
the tests. **None of these is a shipped add-on yet**: each floor is `open` or
`provisional` exactly as the docs hold it, and nothing here was measured on a
box. A real add-on bundle carries its manifest, its compose file and its icon,
signed (§7).

| File | What | Source of its numbers |
|---|---|---|
| `platform.toml` | G1 orchestrator + G2 reverse proxy, which every compose add-on stands on | contract §6 row "G1 + G2" (provisional) |
| `media-hub.toml` | H2 Video and music (Jellyfin) | open (Track H §4, contract §13.2) |
| `mail.toml` | H9 Email (Maddy + Roundcube) | open (Track H §12, contract §13.2) |
