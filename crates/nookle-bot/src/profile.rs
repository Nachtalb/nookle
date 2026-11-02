//! Brings the bot's public profile (name, descriptions, commands, picture) in line at startup.
//! Each part is only written when it differs, so restarts don't hit the rate limits.

use crate::Command;
use teloxide::{prelude::*, types::Me, utils::command::BotCommands};
use tracing::{info, warn};

const NAME: &str = "Nookle";
fn about(user: &str) -> String {
    format!("A private meme collection. Search it from any chat: @{user} cat")
}
fn description(user: &str) -> String {
    format!(
        "Nookle is a private meme collection.\n\n\
         Type @{user} and a few words in any chat to search it: tags, text in the memes, emoji, \
         and image, gif or video.\n\n\
         Only its owner can search and add memes."
    )
}
const AVATAR: &[u8] = include_bytes!("../../../assets/bot-avatar.jpg");

pub async fn sync(bot: &Bot, me: &Me) {
    if let Err(e) = text(bot, me.username()).await {
        warn!("profile text: {e}");
    }
    if let Err(e) = avatar(bot, me.id).await {
        warn!("profile picture: {e:#}");
    }
}

async fn text(bot: &Bot, user: &str) -> ResponseResult<()> {
    let (about, description) = (about(user), description(user));
    if bot.get_my_name().await?.name != NAME {
        bot.set_my_name().name(NAME).await?;
        info!("set name");
    }
    if bot.get_my_short_description().await?.short_description != about {
        bot.set_my_short_description()
            .short_description(about)
            .await?;
        info!("set short description");
    }
    if bot.get_my_description().await?.description != description {
        bot.set_my_description().description(description).await?;
        info!("set description");
    }
    let commands = Command::bot_commands();
    let current = bot.get_my_commands().await?;
    let same = current.len() == commands.len()
        && current.iter().zip(&commands).all(|(a, b)| {
            // bot_commands() keeps the leading "/", Telegram returns them without
            a.command == b.command.trim_start_matches('/') && a.description == b.description
        });
    if !same {
        bot.set_my_commands(commands).await?;
        info!("set commands");
    }
    Ok(())
}

/// Only when the bot has no picture yet: a new upload would add another photo every restart, and
/// one changed by hand in BotFather stays.
async fn avatar(bot: &Bot, me_id: UserId) -> anyhow::Result<()> {
    if bot
        .get_user_profile_photos(me_id)
        .limit(1)
        .await?
        .total_count
        > 0
    {
        return Ok(());
    }
    // setMyProfilePhoto is newer than teloxide 0.17. "./" keeps "bot123:" from parsing as a scheme.
    let url = bot
        .api_url()
        .join(&format!("./bot{}/setMyProfilePhoto", bot.token()))?;
    let form = reqwest::multipart::Form::new()
        .text("photo", r#"{"type":"static","photo":"attach://avatar"}"#)
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(AVATAR)
                .file_name("avatar.jpg")
                .mime_str("image/jpeg")?,
        );
    let r: serde_json::Value = bot
        .client()
        .post(url)
        .multipart(form)
        .send()
        .await?
        .json()
        .await?;
    anyhow::ensure!(r["ok"] == true, "{}", r["description"]);
    info!("set profile picture");
    Ok(())
}
