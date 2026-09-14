# gitlab-ci-exporter

**[English README](README.md)**

![Dashboard Preview](screenshots/Gitlab-CI-Monitor-Dashboards-Grafana.png)

`gitlab-ci-exporter` 用于收集 GitLab CI pipeline 数据，持久化到本地 SQLite 数据库，并通过 HTTP JSON 接口提供监控、Grafana 仪表盘和其他系统集成能力。

## 功能

- 使用 `pipelines.db` 持久化 pipeline 历史。
- 通过 GitLab GraphQL API 增量监控。
- 通过 REST API 执行历史回填，并提供一次性 CLI 回填命令。
- 提供 Grafana 和其他 HTTP 客户端使用的聚合统计接口。
- 按 GitLab pipeline ID 幂等写入，重复回填不会新增重复 pipeline 行。

## 快速开始

先决条件：

- 本地构建需要 Rust 工具链。
- 容器化运行可选 Docker。

本地构建并运行：

```bash
make build
make run
```

默认监听地址为 `0.0.0.0:3000`，配置见 `config.toml`。

使用 Docker：

```bash
make docker-build
make docker-run
```

Makefile 会使用名为 `gitlab-ci-exporter-data` 的 Docker volume 保存数据库，并以只读方式挂载本地 `config.toml`。

## 配置

程序从当前工作目录读取 `config.toml`。

- `[server]`：HTTP 服务的 `host` 和 `port`。
- `[gitlab]`：GitLab `url`、访问 `token`、监控的 `monitor_groups`，以及可选的 `branch_filter_regex`。
- `[poller]`：轮询间隔和启动回填配置。

示例：

```toml
[server]
host = "0.0.0.0"
port = 3000

[gitlab]
url = "https://gitlab.example.com"
token = "your-gitlab-token"
monitor_groups = ["group/subgroup"]
branch_filter_regex = ".*"

[poller]
interval_seconds = 30
backfill_days = 30
```

请勿将访问 token 提交到代码仓库，并限制配置文件的读取权限。

## 历史回填

### 启动时回填

首次安装（数据库中没有 pipeline）时，服务会在启动 HTTP 服务前导入最近 `poller.backfill_days` 天的数据。该配置必须为正数。初次导入完成后，username 补充任务会异步执行，不阻塞正常监控服务。

### 一次性 CLI 回填

已有数据库需要补齐历史数据时，可以执行一次性回填：

```bash
# 使用 config.toml 中的 poller.backfill_days
./target/release/gitlab-ci-exporter backfill

# 回填最近 30 天内更新的 pipeline
./target/release/gitlab-ci-exporter backfill --days 30

# 回填指定 RFC3339 时间之后更新的 pipeline
./target/release/gitlab-ci-exporter backfill \
  --from 2026-08-01T00:00:00Z
```

同时支持 `--backfill-days` 和 `--backfill-from` 别名。该命令从当前工作目录读取 `config.toml` 和 `pipelines.db`，不会清空数据库；重复的 pipeline ID 会执行 upsert，完成后会重建 `daily_stats` 并退出。

查看全部参数：

```bash
./target/release/gitlab-ci-exporter --help
```

## API 接口

- `GET /api/pipelines`：查询已保存的 pipeline。
- `GET /api/projects`：查询数据库中的项目。
- `GET /api/refs`：查询数据库中的 ref。
- `GET /api/stats/summary`：查询总量、时长和成功率聚合统计。
- `GET /api/stats/projects`：查询项目维度统计。
- `GET /api/stats/trend`：查询时间趋势统计。
- `POST /api/refresh_daily_stats`：重建每日聚合统计。

`GET /api/stats/summary` 示例：

```json
{
  "total_count": 1200,
  "avg_duration": 330.7,
  "success_rate": 92.3
}
```

## Grafana

在 Grafana 中导入 `grafana_dashboard.json`，并将 `datasource` 变量配置为可以访问 exporter HTTP 地址的 Infinity datasource。

## Docker 部署

构建镜像：

```bash
docker build -t gitlab-ci-exporter:latest .
```

使用持久化存储和自定义配置运行：

```bash
docker volume create gitlab-ci-exporter-data
docker run -d \
  --name gitlab-ci-exporter \
  --restart unless-stopped \
  -p 3000:3000 \
  -v gitlab-ci-exporter-data:/app \
  -v "$PWD/config.toml:/app/config.toml:ro" \
  gitlab-ci-exporter:latest
```

运行时容器使用非 root 用户，SQLite 数据库保存在 `/app`。

## systemd 部署

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

sudo systemctl daemon-reload
sudo systemctl enable --now gitlab-ci-exporter
sudo journalctl -u gitlab-ci-exporter -f
```

## 故障排查

- Grafana 无数据时，确认 datasource 可以访问 `server.host:server.port`，并检查服务日志。
- 查看日志：`journalctl -u gitlab-ci-exporter -f` 或 `docker logs -f gitlab-ci-exporter`。
- 维护或迁移前请备份 `pipelines.db`。

## 开发

```bash
make test
make fmt
make clippy
```

欢迎提交 issue 或 PR 以改进文档和功能。
