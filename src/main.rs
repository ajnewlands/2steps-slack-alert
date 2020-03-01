use clap::{App, Arg};
use futures::executor::block_on;
use lapin::{options::*, types::FieldTable, Connection, ConnectionProperties, ExchangeKind};
use log::{debug, error, info};
use reqwest::Client;
use serde_json::json;
use std::convert::TryFrom;
use std::fs;
use yaml_rust::YamlLoader;

struct Rabbit {
    conn: Connection,
    chan: lapin::Channel,
    q: lapin::Queue,
    consumer: lapin::Consumer,
}

impl Drop for Rabbit {
    fn drop(&mut self) {
        block_on(self.chan.close(200, "client shut down")).unwrap();
        block_on(self.conn.close(200, "client shut down")).unwrap();
        info!("Shut down");
    }
}

async fn rabbit_connect(ex: &str, q: &str) -> lapin::Result<Rabbit> {
    let addr = std::env::var("AMQP_ADDR").unwrap_or_else(|_| "amqp://127.0.0.1:5672/%2f".into());

    let conn = Connection::connect(&addr, ConnectionProperties::default()).await?;
    let chan = conn.create_channel().await?;

    chan.exchange_declare(
        ex,
        ExchangeKind::Headers,
        ExchangeDeclareOptions::default(),
        FieldTable::default(),
    )
    .await?;

    let queue = chan
        .queue_declare(q, QueueDeclareOptions::default(), FieldTable::default())
        .await?;

    let consumer = chan
        .clone()
        .basic_consume(
            q,
            "my tag",
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;

    info!("Completed rabbit bus initialization");

    Ok(Rabbit {
        conn,
        chan,
        q: queue,
        consumer,
    })
}

fn get_config_path() -> String {
    let default_config = match cfg!(windows) {
        true => "./2steps-slack-alert.conf",
        false => "/etc/opt/remasys/2steps/2steps-slack-alert.conf",
    };

    let matches = App::new("2steps-slack-alert")
        .version("1.0")
        .author("Andrew Newlands")
        .about("Publish 2 Steps alerts to slack")
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .help("set path to configuration file")
                .takes_value(true),
        )
        .get_matches();
    let config = matches.value_of("config").unwrap_or(default_config);

    return String::from(config);
}

struct SlackConfig {
    url: String,
}
impl TryFrom<yaml_rust::Yaml> for SlackConfig {
    type Error = &'static str;

    fn try_from(yaml: yaml_rust::Yaml) -> Result<SlackConfig, Self::Error> {
        let url = (&yaml["slack"]["url"])
            .as_str()
            .ok_or("Configuration missing required Slack URL")?
            .to_string();

        Ok(SlackConfig { url })
    }
}

struct Config {
    slack: SlackConfig,
}

fn read_config(path: &str) -> Result<Config, String> {
    info!("Reading configuration from {}", path);

    let raw =
        fs::read_to_string(path).map_err(|e| format!("Unable to read configuration: {}", e))?;
    let docs = YamlLoader::load_from_str(&raw)
        .map_err(|e| format!("Unable to parse configuration: {}", e))?;

    let slack = SlackConfig::try_from(docs[0].clone())?;
    Ok(Config { slack })
}

#[tokio::main]
async fn main() -> Result<(), String> {
    env_logger::init();

    let cfg = read_config(&get_config_path())?;

    let rabbit = rabbit_connect("2steps", "slack_alerts")
        .await
        .map_err(|e| format!("Failed to initialize rabbit: {:?}", e))?;

    let body = json!({
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": "*Foo Failed*"
                }
            },
            {
                "type":"divider"
            },
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": ">Reason: the fleem is flocked"
                },
                "accessory": {
                    "type": "button",
                    "text": {
                        "type": "plain_text",
                        "emoji": true,
                        "text": "Handle"
                    },
                    "value": "handled something"
                }
            }
        ]
    });

    let client = Client::new();
    let res = client
        .post(&cfg.slack.url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("failed sending to slack: {:?}", e))?;

    match res.status() {
        reqwest::StatusCode::OK => debug!("Message acknowledged by Slack"),
        _ => error!("Slack returned {}", res.status()),
    };

    Ok(())
}
