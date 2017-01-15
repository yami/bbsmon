#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;

extern crate rss;
extern crate reqwest;
extern crate lettre;
extern crate chrono;

#[macro_use]
extern crate serde_derive;

extern crate serde_json;

#[macro_use]
extern crate tera;



use std::io::Read;
use std::io::Write;

use std::fs::File;

use rss::Channel;
use rss::Item;

use lettre::email::EmailBuilder;
use lettre::transport::smtp::SmtpTransportBuilder;
use lettre::transport::smtp::authentication::Mechanism;
use lettre::transport::EmailTransport;

use tera::Tera;

use chrono::DateTime;
use chrono::Local;


mod errors {
    error_chain! {
        foreign_links {
            Io(::std::io::Error);
            Http(::reqwest::Error);
            Rss(::rss::Error);
            Json(::serde_json::Error);
            Render(::tera::Error);
            Mail(::lettre::email::error::Error);
            Tranport(::lettre::transport::smtp::error::Error);
        }
    }
}

use errors::*;


#[derive(Serialize, Debug)]
struct SerItem {
    title: Option<String>,
    link: Option<String>,
    description: Option<String>,
    author: Option<String>,
    pub_date: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Config {
    local_rss: String,
    remote_rss: String,
    
    subject: String,
    from: String,
    to: String,
    password: String,
    server: String,
}

struct RssContext {
    raw: String,
    channel: Channel,
}

impl RssContext {
    pub fn from_url(url: &str) -> Result<RssContext> {
        let resp = reqwest::get(url)?;
        return RssContext::from_reader(resp);
    }

    pub fn from_file(filename: &str) -> Result<RssContext> {
        let reader = File::open(filename)?;
        return RssContext::from_reader(reader);
    }

    pub fn to_file(&self, filename: &str) -> Result<()> {
        let mut writer = File::create(filename)?;
        writer.write_all(self.raw.as_bytes())?;

        return Ok(());
    }

    // return item a vector of Items which are in 'a' but not in 'b'.
    pub fn diff(ctx_a: &RssContext, ctx_b: &RssContext) -> Vec<Item> {
        let a = &ctx_a.channel.items;
        let b = &ctx_b.channel.items;
        
        let mut c = Vec::new();
        
        for item_a in a {
            if !b.contains(item_a) {
                c.push(item_a.clone());
            }
        }

        return c;
    }

    fn from_reader<R: Read>(mut reader: R) -> Result<RssContext> {
        let mut body = String::new();
        reader.read_to_string(&mut body)?;
        
        let channel: rss::Channel = body.parse()?;

        return Ok(RssContext {
            raw: body,
            channel: channel,
        });
    }
}

fn convert_pub_date(old: &Option<String>) -> Option<String> {
    if let &Some(ref date_str) = old {
        if let Ok(date) =  DateTime::parse_from_rfc2822(&date_str) {
            return Some(date
                        .with_timezone(&Local)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string());
        }
    }

    return old.clone();
}

fn convert_to_ser_items(items: &Vec<Item>) -> Vec<SerItem> {
    let mut ser_items = Vec::new();
    
    for item in items {
        ser_items.push(SerItem {
            title: item.title.clone(),
            link: item.link.clone(),
            description: item.description.clone(),
            author: item.author.clone(),
            pub_date: convert_pub_date(&item.pub_date),
        })
    }

    return ser_items;
}

fn load_config(filename: &str) -> Result<Config> {
    let mut reader = File::open(filename)?;

    let mut content = String::new();
    reader.read_to_string(&mut content)?;

    let config: Config = serde_json::from_str(&content)?;
    return Ok(config);
}

fn fetch_diff_items(local: &str, remote: &str) -> Result<(Vec<SerItem>, RssContext)> {
    let new_ctx = RssContext::from_url(remote)?;
    let old_ctx = RssContext::from_file(local)?;

    let new_items = RssContext::diff(&new_ctx, &old_ctx);

    if new_items.len() <= 0 {
        return Ok((Vec::new(), new_ctx));
    } else {
        return Ok((convert_to_ser_items(&new_items), new_ctx));
    }
}

fn render(templates: &str, tmpl_file: &str, items: &Vec<SerItem>) -> Result<String> {
    let tera = compile_templates!(templates);
    
    let mut tctx = tera::Context::new();
    tctx.add("items", &items);

    let content =  tera.render(tmpl_file, tctx)?;

    return Ok(content);
}

fn send_mail(c: &Config, content: &String) -> Result<()> {
    let email = EmailBuilder::new()
        .subject(&c.subject)
        .from(c.from.as_str())
        .to((c.to.as_str(), "BBS Notification Receiver"))
        .header(("Content-Type", "text/html; charset=UTF-8"))
        .body(content)
        .build()?;

    let mut sender = SmtpTransportBuilder::new((c.server.as_str(), 25))?
        .credentials(&c.from, &c.password)
        .smtp_utf8(true)
        .authentication_mechanism(Mechanism::Plain)
        .build();
    
    sender.send(email)?;

    return Ok(());
}

fn run() -> Result<()> {
    let config = load_config("bbsmon.json")?;

    let (items, new_ctx) = fetch_diff_items(&config.local_rss, &config.remote_rss)?;
    if items.len() <= 0 {
        println!("new and old rss are same.");
        return Ok(());
    }
    
    let content = render("templates/**/*", "mail.html", &items)?;

    send_mail(&config, &content)?;
    
    new_ctx.to_file("old-rss.xml")?;

    return Ok(());
}

quick_main!(run);
