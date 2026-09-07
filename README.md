# Lion Assist

A Rust CLI personal assistant with serial task execution, local email tiers, bounded research downloads, model routing, and optional speech. This is an initial implementation: no account credentials or microphone tools are bundled, and no live mailbox changes are made.

## Run locally

```bash
cargo build --locked
cargo run --locked -- status
cargo run --locked -- run < examples/events.jsonl
cargo run --locked -- email examples/critical.eml
cargo run --locked -- list email-
```

Settings are in `config/system.toml`; override with `--config /path/to/system.toml` before the command. Relative paths resolve from the current working directory. `config/limits.toml` is a legacy placeholder and is not loaded. The data directory is private on Unix. Do not point it at an existing general-purpose directory.

`run` reads one JSON object per line, executes it, prints one JSON result, and continues after task errors. It stops at EOF or `{"type":"quit"}`. There is no growing task queue or autonomous model tool execution. `examples/events.jsonl` is an offline demonstration. Supported event types: `status`, `ask`, `email`, `research`, `discord`, `speak`, `listen`, `quit`; see `src/tasks/task.rs` for fields. A process holds the storage lock for its lifetime; stop a watcher before opening another command against the same data directory.

## Storage contract

The maximum setting is **50,000,000,000 bytes (50 decimal GB)**. The default managed-data budget is 49 GB, leaving 1 GB reserved for the installation and overhead. Increase that reservation for voice models or a larger installation. Data writes:

- Count all existing files, directory allocations, and interrupted `.part` files under `data_dir`, using the larger of file length and allocated blocks on Unix.
- Reserve conservatively before creating a temporary file, then sync and rename it. Objects are immutable and deduplicated; a full store rejects new objects and never evicts critical records.
- Reject path traversal, symlinks and special files; use a process lock to serialize writers.
- Keep compression buffers in bounded memory instead of writing an uncompressed disk copy. HTTP responses, MIME inputs, decompression, prompts and model outputs have separate size limits.

**Application checks alone do not guarantee that all program-related physical disk usage will never exceed 50 GB.** Filesystem metadata, allocation behavior, external writers, voice caches, Cargo downloads/builds, backups and redirected output are outside that guarantee. A reserve is headroom, not an OS enforcement mechanism. A filesystem may briefly allocate more than the application estimate before the post-write check detects it.

For the strict requirement, deploy into a dedicated filesystem/volume of at most 50,000,000,000 bytes, or use an OS project quota with a hard block limit rounded *down* from that number. Put the executable, data, all voice assets/caches, temporary files and any program logs in that boundary, and restrict the service to writing there. If development artifacts count, the checkout, Cargo home and target directory must also live inside it. Use memory-backed temporary storage, disable core dumps, and bound or disable external service logs. An ordinary container writable layer or `LimitFSIZE` (per-file) is not an aggregate disk quota. No quota or service has been installed on your machine by this repo.

On restart, orphaned staging files count toward usage. Stop the program and inspect an orphan before manually removing it. A staging-file collision fails closed. Storage expects a private local filesystem with reliable flock/rename semantics; network filesystems and hostile concurrent writers are unsupported. At very large object counts, full directory accounting is intentionally conservative but slower; a quota-backed indexed store would be the next scaling step.

## Email tiers and announcements

```bash
cargo run --locked -- watch-email /path/to/Maildir --once
cargo run --locked -- watch-email /path/to/Maildir
```

Use a locally synchronized Maildir with `new/` and/or `cur/`. A retained directory cursor bounds work per tick. `--once` processes at most one configured batch, not an entire large mailbox. Initial ingestion treats existing unclassified messages as new; later scans suppress duplicates using a content hash. A move from `new/` to `cur/` does not create a duplicate if bytes are unchanged.

| Tier | Local rules |
| --- | --- |
| 1 — Critical | Bank/financial statements, tax documents, fraud/breach alerts, legal notices, emergencies |
| 2 — Important | Authentication codes, password resets, receipts, invoices, order confirmations, appointments |
| 3 — News | Tech/aerospace news, newsletters, digests, mailing-list headers |
| 4 — Other | Everything without a higher-tier match |

Higher-priority rules always win. Rules inspect decoded subject and text MIME parts; attachments are skipped. The stored JSON includes the sender, subject, tier and reason, so classifications can be reviewed with `list` and `show`. These are keyword heuristics, not a guarantee that every critical email is recognized; sender addresses are not authenticated. Edit the phrase lists in `src/tasks/email/mod.rs` to tailor them.

The app stores local tier records and leaves originals in place. Gmail/Outlook OAuth, IMAP synchronization and server-side label/folder updates remain to be implemented once a mailbox provider and desired behavior are selected. Email contents are not sent to a model by this organizer. With voice enabled, newly persisted messages trigger a tier-only announcement; subjects, bodies and authentication codes are not spoken. An announcement failure is reported and is not retried after persistence, avoiding duplicate announcements.

## Aerospace research

```bash
cargo run --locked -- research 'electric propulsion'
cargo run --locked -- list paper-
cargo run --locked -- show paper-OBJECT_HASH.json.gz
```

Searches [arXiv's Atom API](https://github.com/arXiv/arxiv-docs/blob/develop/source/help/api/user-manual.md) and [Crossref's metadata API](https://www.crossref.org/documentation/retrieve-metadata/rest-api/). Each source returns up to `research.max_results`; there is no unlimited crawl or pagination. Requests are serial, spaced at least three seconds apart within a process, with timeout and response-byte caps. Source failures are reported separately; if both fail the command fails. HTTP errors are not automatically retried. Respect `Retry-After` before rerunning after rate limits, and avoid rapid process restarts that reset pacing.

Titles, available abstracts, source identifiers, dates and links are gzip-compressed into one immutable object per source ID. Crossref search is relevance ranked and may return tangential results; metadata and abstracts depend on what publishers deposited. Records keep provenance; duplicate versions across different sources are not merged.

Set `research.download_pdfs = true` to also archive arXiv PDFs as `pdf-HASH.pdf.gz`. Only arXiv's fixed HTTPS host is used, responses must begin with the PDF signature, and the same network and disk limits apply. Large PDFs are rejected. HTTP redirects fail closed. Crossref/publisher full-text retrieval, NASA NTRS, PDF-to-text extraction and semantic indexing are not implemented. Metadata saved before a PDF failure remains usable; rerunning retries a missing PDF. Existing PDFs are skipped. PDF compression is lossless and often saves little because PDFs are already compressed; the default metadata/abstract mode gives much better storage efficiency. Use `gzip -dc data/pdf-HASH.pdf.gz` to stream an archived PDF to a viewer; any redirected export must stay inside the deployment quota.

## Models and Discord

Set `GEMINI_API_KEY` in your environment, then:

```bash
cargo run --locked -- ask 'Summarize what an ion thruster does'
cargo run --locked -- ask --complex 'Compare electric and chemical propulsion for a Mars mission'
cargo run --locked -- summarize /path/to/conversation.txt
cargo run --locked -- discord CHANNEL_ID
```

Simple requests and summaries use the cheap tier; long requests and reasoning keywords such as “compare”, “derive” and “tradeoff” select the advanced tier. `--complex` forces the advanced tier. This is a deterministic heuristic, not a confidence estimator. Models and endpoints are independently configurable for each tier. Defaults use the documented identifiers [`gemini-3-flash-preview`](https://ai.google.dev/gemini-api/docs/models/gemini-3-flash-preview) and [`gemini-3.1-pro-preview`](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-pro-preview). “Google Flash 3.8” was not verified as a supported identifier; replace defaults with models available to your account as needed.

`provider = "gemini"` uses the native generateContent protocol. `provider = "chat_completions"` supports compatible services with `base_url` ending at their API root (e.g. `/v1`), a model name and an environment-variable name for the key. That adapter sends `max_tokens`; choose a service/model supporting that parameter. Localhost HTTP is allowed for local servers; remote endpoints require HTTPS. Provider keys are loaded only when used, and request URLs/bodies/keys are not logged.

Every attempt consumes the per-process call budget, including failures. Prompt/output limits are enforced; no automatic paid retries or fallback occur. This bounds requests, not dollars: configure billing caps at the provider for a monetary guarantee. Model conversations are stateless and answers are not persisted automatically. Text passed to `ask`, `summarize`, `discord` or `listen` goes to the configured model provider. External text is marked as untrusted and models have no action tools.

Discord uses `DISCORD_BOT_TOKEN` and fetches the latest 100 messages from the specified channel, in chronological order for summarization. The bot needs access to the channel, Read Message History and [Message Content intent where required](https://docs.discord.com/developers/resources/message). This is an on-demand summary of message text, not a complete channel archive; attachments, voice channels and live Gateway events are not handled. The app sends no Discord messages.

## Voice

Set `voice.enabled = true`. The default `speak_command` expects locally installed `espeak-ng` and uses its British English synthetic voice:

```bash
cargo run --locked -- speak 'Sir, your research is ready.'
cargo run --locked -- listen
```

For a richer JARVIS-style sound, configure your chosen licensed TTS adapter in `speak_command`. It must read UTF-8 text from stdin, play audio synchronously and exit. The repo does not bundle a character voice or cloned voice model.

`listen_command` must be a trusted executable/argument array that records one utterance, transcribes it and prints only the UTF-8 transcript to stdout. Configure an installed speech recognizer wrapper here before using `listen`; no recognizer or microphone driver is bundled. Listening is on demand. Its transcript is answered using model routing and spoken back; spoken text cannot execute shell commands or modify files. Always-on wake words and spoken feature dispatch are future work.

Commands are launched without a shell. Input/output sizes and wall time are bounded, and on Unix the adapter process group is killed on completion or timeout. Adapters must not detach from their group or daemonize. Speech text is limited to 4000 bytes; transcripts to 16 KiB. Keep all adapter models, caches and scratch files inside the OS storage boundary; application storage accounting cannot police external executables.

## Verify

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover quota rejection, restart accounting, competing locks, orphaned writes, symlink/traversal rejection, input/decompression limits, MIME email tier precedence, deduplication, both research parsers, model routing, a localhost mock provider, event-loop recovery and voice subprocess timeout. Tests use no real accounts or paid APIs. The localhost mock requires permission to bind a local socket in restricted sandboxes.
