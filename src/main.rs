#![feature(proc_macro)]

extern crate rss;
extern crate reqwest;
extern crate lettre;
extern crate chrono;

#[macro_use]
extern crate serde_derive;

extern crate serde_json;

#[macro_use]
extern crate tera;


use std::fmt;

use std::error;
use std::error::Error;

use std::result;
use std::io;
use std::io::Read;
use std::io::Write;

use std::fs::File;

use rss::Channel;
use rss::Item;

use lettre::email::EmailBuilder;
use lettre::transport::smtp::{SecurityLevel, SmtpTransport, SmtpTransportBuilder};
use lettre::transport::smtp::authentication::Mechanism;
use lettre::transport::EmailTransport;

use tera::Tera;

use chrono::DateTime;
use chrono::Local;

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


#[derive(Debug)]
enum MyError {
    Io(io::Error),
    Http(reqwest::Error),
    Rss(rss::Error),
    Json(serde_json::Error),
    Other(String),
}

// TODO: below code are boring, do we have a better way to auto-def these?
impl fmt::Display for MyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            MyError::Io(ref e) => e.fmt(f),
            MyError::Http(ref e) => e.fmt(f),
            MyError::Rss(ref e) => e.fmt(f),
            MyError::Json(ref e) => e.fmt(f),
            MyError::Other(ref s) => write!(f, "other error: {}", s),
        }
    }
}

impl error::Error for MyError {
    fn description(&self) -> &str {
        match *self {
            MyError::Io(ref e) => e.description(),
            MyError::Http(ref e) => e.description(),
            MyError::Rss(ref e) => e.description(),
            MyError::Json(ref e) => e.description(),
            MyError::Other(ref s) => s.as_str(),
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            MyError::Io(ref e) => Some(e),
            MyError::Http(ref e) => Some(e),
            MyError::Rss(ref e) => Some(e),
            MyError::Json(ref e) => Some(e),
            _ => Some(self),
        }
    }
}

impl From<reqwest::Error> for MyError {
    fn from(e: reqwest::Error) -> MyError {
        return MyError::Http(e);
    }
}

impl From<rss::Error> for MyError {
    fn from(e: rss::Error) -> MyError {
        return MyError::Rss(e);
    }
}

impl From<io::Error> for MyError {
    fn from(e: io::Error) -> MyError {
        return MyError::Io(e);
    }
}

impl From<serde_json::Error> for MyError {
    fn from(e: serde_json::Error) -> MyError {
        return MyError::Json(e);
    }
}

type Result<T> = result::Result<T, MyError>;

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
    let tera = compile_templates!("templates/**/*");
    
    let mut tctx = tera::Context::new();
    tctx.add("items", &items);

    match tera.render("mail.html", tctx) {
        Ok(s) => Ok(s),
        Err(e) => Err(MyError::Other(String::from("render failed"))),
    }
}

fn send_mail(c: &Config, content: &String) -> Result<()> {
    let email_builder = EmailBuilder::new()
        .subject(&c.subject)
        .from(c.from.as_str())
        .to((c.to.as_str(), "BBS Notification Receiver"))
        .header(("Content-Type", "text/html; charset=UTF-8"))
        .body(content);

    let email = match email_builder.build() {
        Ok(m) => m,
        Err(e) => return Err(MyError::Other(String::from(e.description()))),
    };

    let sender_builder = match SmtpTransportBuilder::new((c.server.as_str(), 25)) {
        Ok(b) => b,
        Err(e) => return Err(MyError::Other(String::from(e.description()))),
    };

    let mut sender = sender_builder
        .credentials(&c.from, &c.password)
        .smtp_utf8(true)
        .authentication_mechanism(Mechanism::Plain)
        .build();
    
    let result = sender.send(email);

    println!("{:?}", result);

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(MyError::Other(String::from(e.description()))),
    }
}

fn main() {
    let config = load_config("bbsmon.json").unwrap();

    let (items, new_ctx) = fetch_diff_items(&config.local_rss, &config.remote_rss).unwrap();
    if items.len() <= 0 {
        println!("new and old rss are same.");
        return;
    }
    
    let content = render("templates/**/*", "mail.html", &items).unwrap();

    send_mail(&config, &content).unwrap();
    
    new_ctx.to_file("old-rss.xml").unwrap();
}
