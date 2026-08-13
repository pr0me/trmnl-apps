# Berlin Times scheduler

The ARM server owns wall-clock scheduling. GitHub Actions still builds the edition and deploys GitHub Pages, but it is invoked through `workflow_dispatch` instead of GitHub's best-effort `schedule` event.

```text
systemd timer → dispatcher → GitHub workflow_dispatch → generator → Pages → TRMNL polling
                    ↓                    ↓
             public edition check   slot deduplication
```

## Timing and recovery

All timers use `Europe/Berlin`, including daylight-saving transitions.

| Edition | Primary dispatch | Stale-edition recovery | Visibility target |
| --- | ---: | ---: | ---: |
| Morning | 06:00 | 06:20 | 06:45 |
| Evening | 17:00 | 17:20 | 17:45 |

The primary timer always dispatches. The recovery timer reads the public `edition.json` and dispatches exactly one recovery workflow when the expected edition is still absent. If the public read itself fails, recovery dispatches rather than assuming success.

Every request carries a slot such as `2026-08-12-morning`. The workflow checks the deployed edition before doing paid work and skips a slot that is already online. It validates the generated slot again before upload. Workflow-level concurrency serializes overlapping primary and recovery runs, so retries and at-least-once HTTP delivery cannot overwrite the intended slot with a duplicate.

The visibility target includes normal generation, Pages deployment, and the plugin's 15-minute polling interval. The physical device refresh schedule remains independent and must also be short enough to show the updated TRMNL render by the target.

## Credential

Create a fine-grained GitHub personal access token with:

- the account or organization that owns the deployment repository as resource owner;
- access limited to that repository;
- repository permission **Actions: Read and write**;
- an expiry date and rotation reminder.

Do not use a classic `repo` token. The dispatcher needs no Contents permission because the repository and published edition are public. Never place the token in this checkout, a command argument, shell history, a systemd unit, or GitHub Actions.

The only secret-bearing file is `/etc/berlin-times/dispatcher.env` on the scheduling host. It must remain owned by root with mode `0600`. The service uses a transient dynamic user, and systemd reads the environment file before dropping privileges.

## Installation

On the Linux host that will own the schedule, copy the public scheduler bundle from a trusted checkout and run:

```sh
sudo ./ops/berlin-times/install.sh
```

Open the environment file without putting the token in a command:

```sh
sudoedit /etc/berlin-times/dispatcher.env
```

Replace all required placeholders with deployment-specific values:

```text
BERLIN_TIMES_GITHUB_TOKEN=<fine-grained-token>
BERLIN_TIMES_GITHUB_REPOSITORY=OWNER/REPOSITORY
BERLIN_TIMES_EDITION_URL=https://example.github.io/repository/edition.json
```

Then enforce permissions, test one stale-aware dispatch, and enable the four timers:

```sh
sudo chown root:root /etc/berlin-times/dispatcher.env
sudo chmod 0600 /etc/berlin-times/dispatcher.env
sudo systemctl start berlin-times-recover@morning.service
sudo systemctl enable --now berlin-times-morning.timer berlin-times-morning-recovery.timer berlin-times-evening.timer berlin-times-evening-recovery.timer
```

The test is safe: it exits without dispatching when today's morning edition is already online. Otherwise it starts the missing slot once.

## Verification and operation

Confirm calendar expansion, timer state, and recent dispatcher output:

```sh
export BERLIN_TIMES_GITHUB_REPOSITORY=OWNER/REPOSITORY
export BERLIN_TIMES_EDITION_URL=https://example.github.io/repository/edition.json
systemd-analyze calendar '*-*-* 06:00:00 Europe/Berlin' '*-*-* 06:20:00 Europe/Berlin' '*-*-* 17:00:00 Europe/Berlin' '*-*-* 17:20:00 Europe/Berlin'
systemctl list-timers 'berlin-times-*' --all --no-pager
sudo journalctl -u 'berlin-times-*' --since today --no-pager
```

A transport or authentication failure leaves the oneshot unit failed and emits a lower-case error to the journal. GitHub separately reports workflow generation or deployment failures. Inspect both surfaces when `edition.json` is stale:

```sh
systemctl --failed --no-pager
gh run list --repo "$BERLIN_TIMES_GITHUB_REPOSITORY" --workflow edition.yml --limit 10
curl --fail --silent "$BERLIN_TIMES_EDITION_URL" | jq '{edition_name, generated_at}'
```

Manual recovery remains available and uses the same idempotency key:

```sh
gh workflow run edition.yml --repo "$BERLIN_TIMES_GITHUB_REPOSITORY" -f edition_slot="$(TZ=Europe/Berlin date +%F)-morning"
```

To stop external scheduling without removing files:

```sh
sudo systemctl disable --now berlin-times-morning.timer berlin-times-morning-recovery.timer berlin-times-evening.timer berlin-times-evening-recovery.timer
```

After rotating the token, update only `/etc/berlin-times/dispatcher.env`; oneshot services read it on every invocation and need no restart.
