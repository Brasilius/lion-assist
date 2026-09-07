# Running Lion on your PC

Lion now has a resident execution path. It runs configured workflows, records decisions and results, and routes announcements through speech. It does not need a model call when nothing has changed.

## Build and check readiness

```bash
cargo build --release --locked
./target/release/lion-assist doctor
```

`doctor` reports whether local configuration, key environment variables and voice executable paths are present. It does not contact accounts, print secret values, validate credentials or test microphone hardware.

Edit `config/system.toml`. Paths resolve from the working directory; absolute paths are recommended for a service. Keep API keys in the environment, not TOML. The existing cheap and advanced provider adapters can use independent endpoints and credentials.

## Research

Set these keys in the existing `[agent]` section:

```toml
interests = "Electric propulsion, spacecraft thermal control, and reusable launch systems. Prefer new experimental results and explain why each finding matters."
research_queries = ["electric propulsion", "spacecraft thermal control"]
feeds = []
```

Add known RSS/Atom URLs or individual article-page URLs to `feeds`. The agent reads configured sources; it does not recursively crawl arbitrary model-selected URLs. RSS/Atom items are evaluated from their supplied descriptions. Individual HTML pages are parsed into text, excluding scripts, styles and common navigation. It does not execute JavaScript or bypass paywalls. arXiv/Crossref remain the paper-search sources; the original bounded PDF option still applies to arXiv.

Every six hours by default, the agent retrieves candidates. Cheap-model selection compares them with `interests`, summarizes the supplied evidence and explains relevance. Difficult evaluations escalate to the advanced model. Selected items are saved as `reading-HASH.json`; all candidate metadata remains in `paper-HASH.json.gz`. Reading-list entries deduplicate by URL. Source-version URLs are not merged into a semantic identity.

The 15-minute rundown checks what is due. Missed periods coalesce into one catch-up, and unchanged completed candidates do not incur repeat model calls. To trigger a search while running:

```bash
./target/release/lion-assist agent "research ion propulsion"
```

## Email

Point the agent at a locally synchronized Maildir:

```toml
[agent.email]
maildir = "/absolute/path/to/Maildir"
apply_moves = false
review_all = true
policy = "Keep financial, legal and security messages critical. Mark deadlines and personal requests important. Group aerospace newsletters as news. Preserve every message; never delete email."
```

Replace the existing table rather than adding a duplicate. The daemon scans `new/` and `cur/` in bounded batches. Existing messages are included on first ingestion. It sends decoded sender, subject and up to 24 KB of text parts to the cheap model. The advanced model independently reviews every decision by default, with the original evidence. Turning `review_all` off retains strong review for important, critical or ambiguous cases. Keywords only supply a triage signal. Decisions require valid categories, explanations and quotes that actually occur in the message.

With `apply_moves = false`, decisions are recorded without moving originals. Set it to `true` to organize new decisions into Maildir++ folders `.Lion-Critical`, `.Lion-Important`, `.Lion-News` and `.Lion-Other`. Hard-link/unlink operations preserve bytes and flags, avoid overwriting a destination and support crash reconciliation. Source and target must be on the same filesystem. A synchronizer must support these folders and propagate moves if you want server-side organization; Gmail/Outlook OAuth and IMAP synchronization are not bundled.

Completed preview decisions are not silently reapplied when you change settings. To process a reviewed decision after changing policy or enabling moves, use the `reprocess` command below. Its new workflow revision preserves the original activity record.

The activity record contains the proposal, configured policy, move paths and result. A running agent supports:

```bash
./target/release/lion-assist agent '{"command":"undo","job_id":"JOB_ID"}'
./target/release/lion-assist agent 'undo that'
```

Undo restores a completed email move if the original location is still available and the destination message is unchanged. Concurrent mailbox edits or synchronizer renames that make the saved paths unavailable cause a reviewable failure rather than overwriting mail. Critical arrivals bypass voice quiet hours; spoken alerts omit message bodies and subjects.

## Discord

Configure a bot account and the exact channels or thread IDs it may use:

```toml
[agent.discord]
enabled = true
allow_send = false
away = false
channels = ["CHANNEL_ID"]
bot_id = "BOT_USER_ID"
owner_name = "Leo"
reply_policy = "Answer factual questions addressed to the bot. Do not make commitments or disclose private information."
api_base = "https://discord.com/api/v10"
token_env = "DISCORD_BOT_TOKEN"
```

Export `DISCORD_BOT_TOKEN`. The bot needs channel access and permission to read history; sending requires send permission. Configure Message Content intent where required. Channel messages and reply creation follow the [official Discord message API](https://docs.discord.com/developers/resources/message).

On first connection the agent records the latest message ID without answering old history. Subsequent polls backfill missed messages across pages, retaining their position when the job queue fills. Only non-bot, non-webhook messages explicitly mentioning the configured bot are eligible. Thread IDs must be configured explicitly; automatic thread discovery is not implemented.

Enable away mode with `agent "I'm away"`; disable it with `agent "I'm back"`. Away and pause preferences persist across restarts and override their initial TOML defaults. The daemon continues reading cursors while the user is present, so it does not answer those messages later simply because away mode changes.

With `allow_send = false`, eligible jobs produce stored drafts. Set `allow_send = true` to permit new replies while away. Each reply begins with a code-enforced disclosure identifying the user's AI assistant. Mention pings are disabled. The model gets only the bounded channel context and reply policy, not email data or a general private-memory dump. It defers questions requiring personal commitments or unknown private facts.

The sending code records intent before POST and verifies the returned message. A 429 schedules a delayed retry using Discord's [rate-limit guidance](https://docs.discord.com/developers/topics/rate-limits). A transport failure or other uncertain response triggers reconciliation, never a blind repeat POST. If the matching reply cannot be found in a bounded recent-message window, the job enters review. Inspect the channel; use `reconcile` to check again or `dismiss` to acknowledge the unresolved job. This favors avoiding duplicates over guaranteeing a reply after every ambiguous failure. Discord's nonce deduplication is time-limited and is not treated as permanent protection.

You can request a spoken summary with `agent "summarize Discord"`; it uses the first configured channel. A JSON `summarize_discord` command accepts an explicit `channel`.

## Voice

Set `voice.enabled = true` and configure the existing `speak_command`. The default expects `espeak-ng` on PATH. A TTS adapter reads text from stdin, plays it and exits. You can substitute a richer locally installed voice adapter.

For microphone input on Linux, the repository includes `scripts/listen-whisper.sh`. Install ALSA's `arecord`, a built `whisper-cli`, and a local whisper.cpp GGML model. The wrapper records five seconds of mono 16 kHz audio and runs the [whisper.cpp CLI](https://github.com/ggml-org/whisper.cpp/tree/master/examples/cli), then emits only its transcript.

```bash
export LION_WHISPER_MODEL=/absolute/path/to/ggml-base.en.bin
export LION_VOICE_TMP=/absolute/path/to/private/voice-scratch
```

Create the scratch directory with private permissions. Keep models and scratch inside your deployment's storage boundary, **outside `data_dir`** so external transient files cannot race the managed store's accounting. Interrupted adapter execution can leave capture directories there; include that space in the OS quota.

Configure:

```toml
# In the existing [voice] table:
enabled = true
listen_command = ["bash", "scripts/listen-whisper.sh"]
timeout_secs = 30

# In the existing [agent.voice] table:
continuous = true
wake_phrase = "Lion"
```

Examples: “Lion, research electric propulsion,” “Lion, summarize Discord,” “Lion, read it,” “Lion, why is it important?”, “Lion, undo that,” “Lion, I'm away,” “Lion, pause,” and “Lion, resume.” Other utterances become bounded model questions. The wake phrase is checked after transcription; recording is active between playback periods. This is not an acoustic wake-word detector or speaker authentication. Use an empty phrase only if every captured utterance should be treated as input.

Listening and playback share one audio worker so the agent cannot interpret its own speech. Workflows run separately. Speech waits for the current recognition adapter to finish, and the current synchronous playback must finish before another voice command is heard. Stop/pause controls through the local socket remain available during model calls; an already-started remote action cannot be recalled. Continuous listening remains active while workflows are paused so “resume” still works.

Optional `quiet_start_utc` and `quiet_end_utc` defer routine announcements. Critical email bypasses those hours. Up to 100 pending announcements are retained, with older overflow dropped from speech but still represented in job history. Playback is at-most-once: a crash or failed/full speech adapter can skip an announcement; it never replays a mailbox move or Discord reply.

## Run and control

```bash
./target/release/lion-assist daemon
./target/release/lion-assist agent status
./target/release/lion-assist agent pause
./target/release/lion-assist agent resume
./target/release/lion-assist agent rundown
./target/release/lion-assist agent stop
```

`daemon --once` performs one bounded batch and prints the resulting state. It is useful for connector validation; it does not launch the continuous audio worker. A user-private Unix socket provides control without taking the store lock. The default directory is `/tmp/lion-assist-control-<uid>`; override `agent.control_dir` for multiple independent instances. Do not put the socket under `data_dir`.

`agent` returns immediate admission status for commands and a cached status snapshot for `status`. Accepted commands are initially in the process's bounded control queue, not yet durable. Once processed, their jobs are checkpointed; use `status` to see job IDs, results and errors. Pause, presence and stop flags also take effect immediately at the next action boundary. An in-flight HTTP call can run until its configured timeout. The status snapshot updates at job boundaries.

Status includes active/review jobs, the most recent 30 completed jobs, connector errors and pending announcements. Complete activity records remain in `activity-JOB_ID.json`. Stop the daemon before using the original storage commands:

```bash
./target/release/lion-assist list activity-
./target/release/lion-assist show activity-JOB_ID.json
./target/release/lion-assist list reading-
```

Use these control commands to manage review jobs:

```bash
./target/release/lion-assist agent '{"command":"retry","job_id":"JOB_ID"}'
./target/release/lion-assist agent '{"command":"reconcile","job_id":"JOB_ID"}'
./target/release/lion-assist agent '{"command":"dismiss","job_id":"JOB_ID"}'
./target/release/lion-assist agent '{"command":"reprocess","job_id":"JOB_ID"}'
```

`retry` re-evaluates a failed job; it cannot resend an uncertain Discord action. `reconcile` only checks an uncertain delivery again. `dismiss` archives the job with its prior evidence and error, making room in the bounded queue. `reprocess` creates a new email decision from a completed record whose email has not been moved; it is useful after preview or policy changes and never replays Discord sends.

## Budgets, durability and service deployment

The daemon persists daily call and conservative token reservations before each model attempt. `daily_model_calls` and `daily_token_budget` reset at UTC midnight, surviving restarts and backwards clock adjustments. `reserved_email_calls` leaves call capacity for email. Each job also has a call and retry ceiling. Budget exhaustion defers work; repeated failures enter review. These are application request limits, not a provider billing guarantee.

Checkpoint replacement reserves disk for both copies, syncs the new file and renames it atomically. Completed content and activity objects are immutable. Queue state is bounded, but the reading list and historical activity grow until the configured storage budget is reached. At that point writes fail closed; nothing silently evicts records. The README's OS-quota requirement still applies to the complete installation.

For automatic startup, adapt `deploy/lion-assist.service` to your checkout path. Put credentials in `~/.config/lion-assist/environment` with mode 0600; `deploy/environment.example` lists the variables. Then install and enable it yourself:

```bash
mkdir -p ~/.config/systemd/user
cp deploy/lion-assist.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now lion-assist.service
```

The template starts with your user session and restarts on failure. It has not been installed or enabled by the implementation session. Running after logout depends on your system's user-service policy. Logs go to the system journal; configure journal limits or disable them when enforcing a strict whole-installation disk budget. Microphone availability also depends on the active audio session.

## Verification

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The integration suite uses temporary Maildirs, mock model/Discord HTTP servers, and a local daemon socket. It makes no paid API calls and sends no real messages. Credential validity, real mailbox synchronization and actual microphone/playback quality require testing on the configured PC.
