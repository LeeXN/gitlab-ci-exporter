use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Serve,
    Backfill {
        days: Option<i64>,
        from: Option<DateTime<Utc>>,
    },
    Help,
}

pub const USAGE: &str = r#"Usage:
  gitlab-ci-exporter
  gitlab-ci-exporter backfill [--days N | --from RFC3339]

Commands:
  backfill       Fetch historical pipelines into the existing pipelines.db

Backfill options:
  --days N, --backfill-days N
                 Fetch pipelines updated during the last N days.
  --from TIME, --backfill-from TIME
                 Fetch pipelines updated after an RFC3339 timestamp.
                 If omitted, poller.backfill_days from config.toml is used.

General options:
  -h, --help     Show this help.

The command uses the current working directory for config.toml and pipelines.db.
It does not replace or clear the database."#;

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    // The first argument is the executable path.
    let _program = args.next();
    let Some(command) = args.next() else {
        return Ok(Command::Serve);
    };

    match command.as_str() {
        "-h" | "--help" => Ok(Command::Help),
        "backfill" => parse_backfill(args),
        other => Err(format!(
            "unknown command `{other}`\n\n{USAGE}"
        )),
    }
}

fn parse_backfill(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut days = None;
    let mut from = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--days" | "--backfill-days" => {
                if days.is_some() {
                    return Err("backfill days was specified more than once".to_string());
                }
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires a positive integer"))?;
                let parsed = value
                    .parse::<i64>()
                    .map_err(|_| format!("{arg} requires a positive integer, got `{value}`"))?;
                if parsed <= 0 {
                    return Err(format!("{arg} requires a positive integer, got `{value}`"));
                }
                days = Some(parsed);
            }
            "--from" | "--backfill-from" => {
                if from.is_some() {
                    return Err("backfill start time was specified more than once".to_string());
                }
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires an RFC3339 timestamp"))?;
                let parsed = DateTime::parse_from_rfc3339(&value)
                    .map_err(|e| format!("{arg} requires an RFC3339 timestamp: {e}"))?
                    .with_timezone(&Utc);
                from = Some(parsed);
            }
            other => {
                return Err(format!(
                    "unknown backfill option `{other}`\n\n{USAGE}"
                ));
            }
        }
    }

    if days.is_some() && from.is_some() {
        return Err("use either --days or --from, not both".to_string());
    }

    Ok(Command::Backfill { days, from })
}
