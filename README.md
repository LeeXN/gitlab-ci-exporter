# gitlab-ci-exporter

![Dashboard Preview](screenshots/Gitlab-CI-Monitor-Dashboards-Grafana.png)

gitlab-ci-exporter collects GitLab CI pipeline metrics, persists them in a local SQLite database, and exposes HTTP endpoints for monitoring, dashboards and integrations. This README provides quick start steps, configuration details, and explains the exporter’s unique historical backfill capability.

**Highlights**
- Lightweight HTTP exporter with built-in persistence (`pipelines.db`).
- Grafana-friendly JSON endpoints (works well with Infinity datasource).
- Historical backfill: can fetch and import past pipeline history from GitLab to fill missing data.

## Quick Start

Prerequisites
- Rust toolchain (for building locally)
- Docker (optional)

Build and run locally

```bash
make build
make run
```

Default server: 0.0.0.0:3000 (see `config.toml`).

Run with Docker

```bash
make docker-build
make docker-run
# or: docker run --rm -p 3000:3000 -v $(pwd)/config.toml:/app/config.toml gitlab-ci-exporter:latest
```

## Features

- Metrics & endpoints: exposes JSON APIs for pipelines, projects and aggregated statistics.
- Persistence: stores pipeline records in `pipelines.db` (SQLite) so historical metrics are available across restarts.
- Historical Backfill: configurable mode to fetch past pipelines from GitLab and populate the local DB — useful for initial import or recovering missed history.

## Historical Backfill (important)

What it does
- Backfill fetches pipelines from GitLab for configured groups/projects and saves them into `pipelines.db`, enabling historical charts and accurate trend analysis.

Where to configure
- The backfill options live in the `[poller]` section of `config.toml` (see below).

Typical options

What it does
- When configured, the exporter can import past pipelines from GitLab into the local `pipelines.db` to provide historical metrics.

Where to configure
- The backfill option is configured in the `[poller]` section of `config.toml`.

Typical option
- `backfill_days` (integer): enable initial backfill and specify how many days of history to import (for example `30` to import the last 30 days).

Behavior notes
- The importer runs only on initial startup when there is no local database file or when the stored history is empty. In that case the exporter will fetch historical pipelines based on `backfill_days` and write them into `pipelines.db` before starting the HTTP service.
- After historical pipelines are written the HTTP service starts. Enrichment tasks (for example filling missing `username` fields) may run asynchronously and will not block normal monitoring once the service is up.

Usage guidance
- Use backfill during the first deployment to populate historical data; disable or omit it for regular runs.
- Backfill consumes GitLab API quota — choose a reasonable `backfill_days` value and monitor API limits.

## Configuration

Place `config.toml` in the process working directory. Key sections:

- `[server]` — `host` and `port` for the HTTP server.
- `[gitlab]` — `url`, `token`, `monitor_groups` (or projects list).
- `[poller]` — controls polling interval and backfill settings (see above).

Example: see the repository `config.toml` for default values and comments.

## API Endpoints (examples)

- `GET /api/stats/summary` — aggregated counts and rates.
- `GET /api/pipelines` — list of stored pipelines.
- `GET /api/projects` — projects being monitored.

Example responses (masking applied):

`GET /api/stats/summary`

```json
{
	"total_count": 1200,
	"avg_duration": 330.7,
	"success_rate": 92.3
}
```

`GET /api/pipelines`

```json
[
	{
		"id": 1234,
		"project_name": "org/project-****",
		"ref": "main",
		"status": "success",
		"created_at": "2025-12-18T12:34:56Z",
		"finished_at": "2025-12-18T12:37:30Z",
		"duration": 154
	}
]
```

## Grafana dashboard

Import `grafana_dashboard.json` (Dashboard → Import). The dashboard uses the Infinity datasource plugin (`yesoreyeram-infinity-datasource`) to query the exporter HTTP APIs. After import, configure the dashboard variable `datasource` to point to your Infinity datasource.

## Makefile targets

- `make build` — build release binary
- `make run` — run using `cargo run --release`
- `make docker-build` / `make docker-run`
- `make test`, `make fmt`, `make clippy`, `make clean`

## Deployment examples

systemd (example)

```bash
sudo mkdir -p /var/lib/gitlab-ci-exporter
sudo cp target/release/gitlab-ci-exporter /usr/local/bin/gitlab-ci-exporter
sudo cp config.toml /var/lib/gitlab-ci-exporter/config.toml
sudo useradd --system --no-create-home --shell /usr/sbin/nologin gitlab-ci-exporter || true
sudo chown -R gitlab-ci-exporter:gitlab-ci-exporter /var/lib/gitlab-ci-exporter
sudo tee /etc/systemd/system/gitlab-ci-exporter.service > /dev/null <<'EOF'
[Unit]
Description=GitLab CI Exporter
After=network.target

[Service]
User=gitlab-ci-exporter
Group=gitlab-ci-exporter
WorkingDirectory=/var/lib/gitlab-ci-exporter
ExecStart=/usr/local/bin/gitlab-ci-exporter
Restart=on-failure
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload && sudo systemctl enable --now gitlab-ci-exporter
sudo journalctl -u gitlab-ci-exporter -f
```

Docker example

```bash
docker build -t gitlab-ci-exporter:latest .
docker run -d --name gitlab-ci-exporter -p 3000:3000 -v /opt/gitlab-ci-exporter/data:/app --restart unless-stopped gitlab-ci-exporter:latest
```

## Troubleshooting

- If Grafana shows no data, confirm the Infinity datasource can reach `server.host:server.port` and the exporter is running.
- Check logs with `journalctl -u gitlab-ci-exporter -f` or `docker logs -f gitlab-ci-exporter`.
- `pipelines.db` stores persisted pipelines — back it up to preserve history.

## Contributing

Please open issues or pull requests for bugs and improvements.

