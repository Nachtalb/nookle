use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use nookle_bot::{
    api::{Api, Upload},
    text,
};
use nookle_core::model::{Analysis, DupMatch, ImportRequest, Item, Status};
use teloxide::{
    net::Download,
    payloads::{
        AnswerCallbackQuerySetters, AnswerInlineQuerySetters, EditMessageTextSetters,
        SendMessageSetters,
    },
    prelude::*,
    types::{
        FileId, FileMeta, InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup,
        LinkPreviewOptions, Me, MessageEntityKind, MessageOrigin, ReplyParameters, User,
    },
    utils::command::BotCommands,
};
use tokio::io::AsyncWriteExt;
use tracing::warn;

const CLOUD_DOWNLOAD_CAP: u32 = 20 * 1024 * 1024;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    Start,
    Help,
}

#[derive(Clone)]
struct Cfg {
    allowed: Arc<Vec<u64>>,
    cloud: bool,
}

impl Cfg {
    fn allows(&self, user: &User) -> bool {
        self.allowed.contains(&user.id.0)
    }
}

#[derive(Clone)]
struct Media {
    file_id: FileId,
    size: u32,
    filename: String,
}

#[derive(Clone)]
struct PendingUpload {
    media: Media,
    sources: Vec<String>,
    tags: Vec<String>,
}

/// Near-duplicate uploads waiting for "Add anyway"; callback data only holds the key.
#[derive(Clone, Default)]
struct Pending {
    map: Arc<Mutex<HashMap<u64, PendingUpload>>>,
    next: Arc<AtomicU64>,
}

impl Pending {
    fn put(&self, up: PendingUpload) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let mut map = self.map.lock().unwrap();
        map.retain(|k, _| k + 1000 > n);
        map.insert(n, up);
        n
    }

    fn take(&self, n: u64) -> Option<PendingUpload> {
        self.map.lock().unwrap().remove(&n)
    }
}

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} must be set"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let api = Api::new(&env("NOOKLE_API")?, &env("NOOKLE_KEY")?);
    let allowed = env("ALLOWED_USER_IDS")?
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse().context("ALLOWED_USER_IDS"))
        .collect::<Result<Vec<u64>>>()?;
    let api_url = std::env::var("TELOXIDE_API_URL").ok();
    let cfg = Cfg {
        allowed: Arc::new(allowed),
        cloud: api_url.is_none(),
    };

    // teloxide's default client times out after 17 s, which kills big downloads.
    let client = teloxide::net::default_reqwest_settings()
        .timeout(Duration::from_secs(600))
        .build()?;
    let mut bot = Bot::with_client(env("BOT_TOKEN")?, client);
    if let Some(url) = api_url {
        bot = bot.set_api_url(url.parse()?);
    }
    tracing::info!(api = %bot.api_url(), "starting");

    let handler = dptree::entry()
        .branch(Update::filter_inline_query().endpoint(inline))
        .branch(Update::filter_chosen_inline_result().endpoint(chosen))
        .branch(Update::filter_callback_query().endpoint(callback))
        .branch(
            Update::filter_message()
                .filter(|m: Message, cfg: Cfg| m.from.as_ref().is_some_and(|u| cfg.allows(u)))
                .branch(dptree::entry().filter_command::<Command>().endpoint(start))
                .branch(dptree::endpoint(message)),
        );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![api, cfg, Pending::default()])
        .distribution_function(|_| None::<()>)
        .default_handler(|_| async {})
        .error_handler(LoggingErrorHandler::with_custom_text(
            "update handler error",
        ))
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    Ok(())
}

async fn inline(bot: Bot, q: InlineQuery, api: Api, cfg: Cfg) -> ResponseResult<()> {
    let mut results = Vec::new();
    let mut next = String::new();
    if cfg.allows(&q.from) {
        let cursor = (!q.offset.is_empty()).then_some(q.offset.as_str());
        match api.search(q.query.trim(), cursor, 50).await {
            Ok(page) => {
                results = page.items.iter().filter_map(text::inline_result).collect();
                next = text::next_offset(page.next_cursor);
            }
            Err(e) => warn!("search {:?}: {e:#}", q.query),
        }
    }
    bot.answer_inline_query(q.id, results)
        .is_personal(true)
        .cache_time(5)
        .next_offset(next)
        .await?;
    Ok(())
}

async fn chosen(r: ChosenInlineResult, api: Api, cfg: Cfg) -> ResponseResult<()> {
    if cfg.allows(&r.from)
        && let Err(e) = api.hit(&r.result_id).await
    {
        warn!("hit {}: {e:#}", r.result_id);
    }
    Ok(())
}

async fn start(bot: Bot, msg: Message, me: Me) -> ResponseResult<()> {
    let help = format!(
        "Send me a photo, GIF, video, sticker or file to add it to Nookle. \
         #words in the caption become tags, links become sources.\n\n\
         Reply to a confirmation with tags (😏 smug -wrong) to edit them.\n\n\
         Send a link (x.com posts too) to import its media.\n\n\
         Search in any chat: @{} cat",
        me.username()
    );
    bot.send_message(msg.chat.id, help)
        .link_preview_options(no_preview())
        .await?;
    Ok(())
}

async fn message(
    bot: Bot,
    msg: Message,
    me: Me,
    api: Api,
    cfg: Cfg,
    pending: Pending,
) -> ResponseResult<()> {
    let caption = msg.caption().or(msg.text()).unwrap_or_default();
    let (tags, mut sources) = text::parse_caption(caption);
    let entities = msg
        .caption_entities()
        .or(msg.entities())
        .unwrap_or_default();
    for e in entities {
        if let MessageEntityKind::TextLink { url } = &e.kind
            && !sources.contains(&url.to_string())
        {
            sources.push(url.to_string());
        }
    }

    if let (Some(to), Some(words), None) = (msg.reply_to_message(), msg.text(), media(&msg))
        && to.from.as_ref().is_some_and(|u| u.id == me.id)
    {
        return retag(&bot, &msg, to, words, &api).await;
    }
    let media = match media(&msg) {
        Some(Ok(m)) => m,
        Some(Err(why)) => return reply(&bot, &msg, why).await,
        None if msg.text().is_some() => return import(&bot, &msg, &api, sources, tags).await,
        None => return Ok(()),
    };
    if cfg.cloud && media.size > CLOUD_DOWNLOAD_CAP {
        return reply(
            &bot,
            &msg,
            "Too big: bots can only download files up to 20 MB from Telegram. Send a link instead.",
        )
        .await;
    }
    if let Some(MessageOrigin::Channel {
        chat, message_id, ..
    }) = msg.forward_origin()
        && let Some(user) = chat.username()
    {
        sources.insert(0, format!("https://t.me/{user}/{}", message_id.0));
    }

    let status = bot
        .send_message(msg.chat.id, "⏳ Downloading…")
        .reply_parameters(ReplyParameters::new(msg.id))
        .link_preview_options(no_preview())
        .await?;
    let up = PendingUpload {
        media,
        sources,
        tags,
    };
    ingest(&bot, &api, &pending, &status, up, false).await;
    Ok(())
}

/// A text reply to one of our confirmations edits the tags of the items it links.
async fn retag(
    bot: &Bot,
    msg: &Message,
    to: &Message,
    words: &str,
    api: &Api,
) -> ResponseResult<()> {
    let ids = text::item_ids(to.text().unwrap_or_default());
    // The items' current tags let `-rock lee` remove a multi-word tag without quotes.
    let mut items = Vec::new();
    for id in &ids {
        items.push((id, api.item(id).await));
    }
    let existing: Vec<String> = items
        .iter()
        .filter_map(|(_, r)| r.as_ref().ok())
        .flat_map(|it| it.tags.iter().cloned())
        .collect();
    let (add, remove) = text::parse_tag_reply(words, &existing);
    if ids.is_empty() || (add.is_empty() && remove.is_empty()) {
        return reply(
            bot,
            msg,
            "Reply to a confirmation with tags to add, -tag to remove. Commas or \"quotes\" for tags with spaces.",
        )
        .await;
    }
    let mut out = Vec::new();
    let mut last = None;
    for (id, item) in items {
        let res = async {
            let tags: Vec<String> = item?
                .tags
                .into_iter()
                .filter(|t| !remove.contains(t))
                .chain(add.iter().cloned())
                .collect();
            api.set_tags(id, &tags).await
        }
        .await;
        out.push(match res {
            Ok(item) => {
                let line = format!("🏷 {id}: {}", item.tags.join(", "));
                last = Some(item);
                line
            }
            Err(e) => format!("❌ {id}: {e:#}"),
        });
    }
    // A single upload confirmation shows the tags, so keep it current.
    if let (1, Some(item), true) = (
        ids.len(),
        last,
        to.text().is_some_and(|t| t.starts_with('✅')),
    ) {
        edit(bot, to, text::item_summary(&item), tag_keyboard(&item)).await;
    }
    reply(bot, msg, &out.join("\n")).await
}

async fn reply(bot: &Bot, msg: &Message, text: &str) -> ResponseResult<()> {
    bot.send_message(msg.chat.id, text)
        .reply_parameters(ReplyParameters::new(msg.id))
        .link_preview_options(no_preview())
        .await?;
    Ok(())
}

fn media(msg: &Message) -> Option<Result<Media, &'static str>> {
    let m = |meta: &FileMeta, name: Option<&String>, default: &str| {
        Ok(Media {
            file_id: meta.id.clone(),
            size: meta.size,
            filename: name.cloned().unwrap_or_else(|| default.into()),
        })
    };
    if let Some(sizes) = msg.photo() {
        let best = sizes.iter().max_by_key(|s| s.width * s.height)?;
        return Some(m(&best.file, None, "photo.jpg"));
    }
    if let Some(a) = msg.animation() {
        return Some(m(&a.file, a.file_name.as_ref(), "animation.mp4"));
    }
    if let Some(v) = msg.video() {
        return Some(m(&v.file, v.file_name.as_ref(), "video.mp4"));
    }
    if let Some(s) = msg.sticker() {
        return Some(if s.is_animated() {
            Err("Animated (.tgs) stickers aren't supported, only static and video stickers.")
        } else if s.is_video() {
            m(&s.file, None, "sticker.webm")
        } else {
            m(&s.file, None, "sticker.webp")
        });
    }
    if let Some(d) = msg.document() {
        let ok = d
            .mime_type
            .as_ref()
            .is_none_or(|t| matches!(t.type_().as_str(), "image" | "video"));
        return Some(if ok {
            m(&d.file, d.file_name.as_ref(), "file")
        } else {
            Err("Only images and videos can be added.")
        });
    }
    None
}

async fn import(
    bot: &Bot,
    msg: &Message,
    api: &Api,
    urls: Vec<String>,
    tags: Vec<String>,
) -> ResponseResult<()> {
    if urls.is_empty() {
        return reply(bot, msg, "Send me media or a link. /help").await;
    }
    let status = bot
        .send_message(msg.chat.id, "⏳ Importing…")
        .reply_parameters(ReplyParameters::new(msg.id))
        .link_preview_options(no_preview())
        .await?;
    let mut out = Vec::new();
    for url in &urls {
        let req = ImportRequest {
            url: url.clone(),
            tags: tags.clone(),
            ..Default::default()
        };
        let res = match api.import(&req).await {
            Ok(r) => text::import_summary(&r),
            Err(e) => format!("❌ {e:#}"),
        };
        out.push(if urls.len() > 1 {
            format!("{url}\n{res}")
        } else {
            res
        });
    }
    edit(bot, &status, out.join("\n\n"), None).await;
    Ok(())
}

async fn callback(
    bot: Bot,
    q: CallbackQuery,
    api: Api,
    cfg: Cfg,
    pending: Pending,
) -> ResponseResult<()> {
    if let Some(data) = q.data.as_deref().filter(|d| d.starts_with("tag")) {
        let data = data.to_string();
        return tag_button(&bot, q, &data, &api, &cfg).await;
    }
    let key = q
        .data
        .as_deref()
        .and_then(|d| d.strip_prefix("force:")?.parse().ok());
    let (Some(key), Some(status), true) = (key, q.regular_message().cloned(), cfg.allows(&q.from))
    else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let Some(up) = pending.take(key) else {
        bot.answer_callback_query(q.id)
            .text("Expired, send it again.")
            .await?;
        return Ok(());
    };
    bot.answer_callback_query(q.id).await?;
    edit(&bot, &status, "⏳ Downloading…".into(), None).await;
    ingest(&bot, &api, &pending, &status, up, true).await;
    Ok(())
}

/// Download from Telegram, upload to Nookle, and turn `status` into the result.
async fn ingest(
    bot: &Bot,
    api: &Api,
    pending: &Pending,
    status: &Message,
    up: PendingUpload,
    force: bool,
) {
    let (text, markup) = match upload(bot, api, status, &up, force).await {
        Ok(Upload::Created(item)) => {
            edit(bot, status, "⚙️ Processing…".into(), None).await;
            match wait_ready(api, *item).await {
                Ok(item) if item.status == Status::Ready && item.analysis == Analysis::Pending => {
                    edit(bot, status, text::item_summary(&item), tag_keyboard(&item)).await;
                    // The tagger runs on another machine; show its tags once they land.
                    let item = wait_tagged(api, item).await;
                    (text::item_summary(&item), tag_keyboard(&item))
                }
                Ok(item) => (text::item_summary(&item), tag_keyboard(&item)),
                Err(e) => (format!("❌ {e:#}"), None),
            }
        }
        Ok(Upload::Duplicate {
            dup_match,
            distance,
            existing,
        }) => {
            let text = text::duplicate_text(dup_match, distance, &existing);
            let markup = (dup_match == DupMatch::Near && !force).then(|| {
                let key = pending.put(up);
                InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
                    "Add anyway",
                    format!("force:{key}"),
                )]])
            });
            (text, markup)
        }
        Err(e) => (format!("❌ {e:#}"), None),
    };
    edit(bot, status, text, markup).await;
}

async fn upload(
    bot: &Bot,
    api: &Api,
    status: &Message,
    up: &PendingUpload,
    force: bool,
) -> Result<Upload> {
    static N: AtomicU64 = AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "nookle-bot-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let res = async {
        let path = fetch(bot, &up.media.file_id, &tmp).await?;
        edit(bot, status, "⬆️ Uploading…".into(), None).await;
        api.upload(&path, &up.media.filename, &up.sources, &up.tags, force)
            .await
    }
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    res
}

/// The local Bot API server's own path when it is readable from here, else
/// `tmp` after an HTTP download.
async fn fetch(bot: &Bot, id: &FileId, tmp: &Path) -> Result<PathBuf> {
    let file = bot.get_file(id.clone()).await?;
    if file.path.starts_with('/') && tokio::fs::try_exists(&file.path).await.unwrap_or(false) {
        return Ok(file.path.into());
    }
    let mut dst = tokio::fs::File::create(tmp).await?;
    bot.download_file(&file.path, &mut dst).await?;
    dst.flush().await?;
    Ok(tmp.to_path_buf())
}

async fn wait_ready(api: &Api, mut item: Item) -> Result<Item> {
    for _ in 0..600 {
        if item.status != Status::Processing {
            return Ok(item);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        item = api.item(&item.id).await?;
    }
    anyhow::bail!("still processing after 10 min: {}", item.page_url)
}

/// Poll until the tagger has reported, for at most 5 minutes; errors keep the last state.
async fn wait_tagged(api: &Api, mut item: Item) -> Item {
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        match api.item(&item.id).await {
            Ok(i) if i.analysis != Analysis::Pending => return i,
            Ok(i) => item = i,
            Err(e) => warn!("poll tags {}: {e:#}", item.id),
        }
    }
    item
}

/// One button per tag (4 per row) and an apply row: tapping a tag marks it with ❌,
/// apply removes the marked ones. The marks live in the button labels, so no state is kept.
fn tag_keyboard(item: &Item) -> Option<InlineKeyboardMarkup> {
    if item.status != Status::Ready || item.tags.is_empty() {
        return None;
    }
    Some(keyboard(
        text::tag_rows(&item.tags)
            .into_iter()
            .map(|r| r.into_iter().collect())
            .collect(),
    ))
}

/// Builds the markup from label rows; button `i` (row-major) toggles with `tag:i`.
fn keyboard(rows: Vec<Vec<String>>) -> InlineKeyboardMarkup {
    let mut i = 0;
    let mut out: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for row in &rows {
        out.push(
            row.iter()
                .map(|l| {
                    i += 1;
                    InlineKeyboardButton::callback(l.clone(), format!("tag:{}", i - 1))
                })
                .collect(),
        );
    }
    let marked = text::marked_tags(rows.iter().flatten().map(String::as_str)).len();
    out.push(vec![InlineKeyboardButton::callback(
        text::apply_label(marked),
        "tagapply",
    )]);
    InlineKeyboardMarkup::new(out)
}

/// Tag button rows of a message's keyboard (without the apply row).
fn tag_labels(msg: &Message) -> Vec<Vec<String>> {
    let Some(kb) = msg.reply_markup() else {
        return Vec::new();
    };
    kb.inline_keyboard
        .iter()
        .map(|row| {
            row.iter()
                .filter(|b| {
                    matches!(&b.kind, InlineKeyboardButtonKind::CallbackData(d) if d.starts_with("tag:"))
                })
                .map(|b| b.text.clone())
                .collect::<Vec<_>>()
        })
        .filter(|r: &Vec<String>| !r.is_empty())
        .collect()
}

async fn tag_button(
    bot: &Bot,
    q: CallbackQuery,
    data: &str,
    api: &Api,
    cfg: &Cfg,
) -> ResponseResult<()> {
    let (Some(msg), true) = (q.regular_message().cloned(), cfg.allows(&q.from)) else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let mut rows = tag_labels(&msg);
    if let Some(i) = data
        .strip_prefix("tag:")
        .and_then(|i| i.parse::<usize>().ok())
    {
        if let Some(label) = rows.iter_mut().flatten().nth(i) {
            *label = text::toggle_label(label);
        }
        bot.answer_callback_query(q.id).await?;
        if let Err(e) = bot
            .edit_message_reply_markup(msg.chat.id, msg.id)
            .reply_markup(keyboard(rows))
            .await
        {
            warn!("toggle tag: {e}");
        }
        return Ok(());
    }
    let remove = text::marked_tags(rows.iter().flatten().map(String::as_str));
    let id = text::item_ids(msg.text().unwrap_or_default())
        .into_iter()
        .next();
    let (Some(id), false) = (id, remove.is_empty()) else {
        bot.answer_callback_query(q.id)
            .text("Tap the tags to remove first.")
            .await?;
        return Ok(());
    };
    let res = async {
        let item = api.item(&id).await?;
        let tags: Vec<String> = item
            .tags
            .into_iter()
            .filter(|t| !remove.contains(t))
            .collect();
        api.set_tags(&id, &tags).await
    }
    .await;
    match res {
        Ok(item) => {
            bot.answer_callback_query(q.id)
                .text(format!("Removed {}", remove.join(", ")))
                .await?;
            edit(bot, &msg, text::item_summary(&item), tag_keyboard(&item)).await;
        }
        Err(e) => {
            bot.answer_callback_query(q.id)
                .text(format!("❌ {e:#}"))
                .show_alert(true)
                .await?;
        }
    }
    Ok(())
}

async fn edit(bot: &Bot, msg: &Message, text: String, markup: Option<InlineKeyboardMarkup>) {
    let mut req = bot
        .edit_message_text(msg.chat.id, msg.id, text)
        .link_preview_options(no_preview());
    if let Some(m) = markup {
        req = req.reply_markup(m);
    }
    if let Err(e) = req.await {
        warn!("edit status: {e}");
    }
}

/// Replies carry item and source links; their previews would bury the text.
fn no_preview() -> LinkPreviewOptions {
    LinkPreviewOptions {
        is_disabled: true,
        url: None,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: false,
    }
}
