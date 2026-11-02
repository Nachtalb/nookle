# Nookle userscript

Adds an "Add to Nookle" button to the action bar of every x.com / twitter.com
post with a photo, video or GIF. A click sends the post URL to
`POST /api/items/import`; the backend downloads all media itself. A toast
shows what was added and what was already there, with links to the items. A
green checkmark marks posts added in this session.

## Install

1. Install Tampermonkey or Violentmonkey (desktop or Edge on Android).
2. Open [nookle.user.js](https://raw.githubusercontent.com/Nachtalb/nookle/main/userscript/nookle.user.js) and let the manager install it. It updates
   itself from there whenever `@version` goes up.
3. Open x.com and click the button on any post. The first click asks for an
   rw API key (`nk_...`, create one in the Nookle web UI).

The manager's menu has "Set Nookle API key" and "Set Nookle API URL"
(default `https://n.naa.gg`). For the mock, set the URL to
`http://127.0.0.1:8787`; `@connect` allows `n.naa.gg`, `localhost` and
`127.0.0.1`.

## Tests

```sh
bun test userscript/test/
```

The `*-article.html` fixtures are real (logged-out) x.com post markup with
classes stripped; `import-response.json` is a real mock import response.
