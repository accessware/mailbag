# mailbag

Problem reports from desktop apps, emailed to the developer. The app sends a report to a small server you run, and the server emails it to you. The app never holds a mail password, and the server only ever emails one address, so it's no use to a spammer.

| Crate | What it does |
| --- | --- |
| `mailbag` | The report: what happened, the app and system it happened on, and the logs. Sends it to a server. |
| `mailbag-server` | Takes reports over HTTP and emails each one, with its logs attached. |
| `mailbag-wx` | A Report a Problem dialog for wxDragon apps, offered at the next start after a crash. |

## Sending a report

```rust
let report = mailbag::Report::new("Paperback", "1.2.0", description)
	.with_commit(commit)
	.with_reply_to(address)
	.crashed(crashed_last_run)
	.with_log("paperback.log", &log);
report.send("https://reports.example.com/v1/report")?;
```

- A log longer than 1 MB keeps its end, where the lines leading up to a problem are.
- `to_text` gives the whole report as text, to show the user before sending, or to save to a file if sending fails.
- `validate` checks the same limits the server does.

The `send` feature, on by default, adds `Report::send`. Leave it off to use the report type alone.

To check a server end to end, run `cargo run --example send -- https://reports.example.com/v1/report`.

## Adding the dialog to a wxDragon app

`mailbag-wx` gives a wxDragon app a Report a Problem dialog, and offers it at the next start after a crash.

```rust
let config_dir = homeport::Homeport::new("Paperback").create_config_dir()?;
let session = mailbag_wx::Session::begin(&config_dir, "paperback");
// After the main window shows:
let reporter = mailbag_wx::Reporter::new("Paperback", "1.2.0", "https://reports.example.com/v1/report")
	.with_log(config_dir.join("paperback.log"))
	.with_log(config_dir.join("paperback.log.1"));
reporter.offer_after_crash(&frame, &session);
// From a Help menu item:
reporter.show(&frame, false);
// As the app exits normally:
session.end();
```

Pass the folder the app keeps its settings in, such as the one [homeport](https://github.com/accessware/homeport) finds, so a portable copy keeps these files beside it.

### The dialog

- It asks what happened, and for an email address if the user wants a reply.
- Include the log is checked when a log exists. Unchecking it leaves the logs out.
- Preview shows the whole report, exactly as it will be sent.
- Send shows a progress window, then a thank-you. An email address whose domain doesn't take email is refused, and the user is put back in that field to fix it.
- If sending fails, or the server's daily limit is reached, the user can save the report to a text file and send it another way.
- Every string is translated with patois 0.3. The app must use the same version of patois.

### After a crash

`Session::begin` marks the run as running, and `end` clears the mark. A mark left at the next start means the last run crashed, was killed, or lost power, and `offer_after_crash` asks whether to report the problem. A report sent from there is marked as a crash.

- The marker stays locked while the app runs, so a second copy of the app doesn't mistake the first for a crash.
- The question has a Don't ask again check box. The session remembers it in a file next to the marker, so the app doesn't need a setting for it. To offer a setting that turns the question back on, call `session.set_asks_after_crash(true)`.

To try the dialog, run `cargo run -p mailbag-wx --example report -- https://reports.example.com/v1/report`. Choose Simulate a Crash, then run it again, to see the question asked after a crash.

## Running the server

The server listens on `127.0.0.1:8025` and expects a reverse proxy such as Caddy to provide HTTPS. It's configured by environment variables:

| Variable | Default | What it sets |
| --- | --- | --- |
| `MAILBAG_SMTP_USER` | Required | The account that sends the emails. |
| `MAILBAG_SMTP_PASSWORD_FILE` or `MAILBAG_SMTP_PASSWORD` | Required | Its password, such as a Gmail app password. The file form is for a container secret. |
| `MAILBAG_TO` | Required | Where reports go. |
| `MAILBAG_APPS` | Required | The apps whose reports are taken, separated by commas, such as `paperback,fedra`. Case doesn't matter. |
| `MAILBAG_FROM` | `mailbag <MAILBAG_SMTP_USER>` | The sender. Gmail replaces it with the account that signed in. |
| `MAILBAG_SMTP_HOST` | `smtp.gmail.com` | The SMTP server. |
| `MAILBAG_SMTP_PORT` | `587` | Its port. 465 starts with TLS; any other port uses STARTTLS. |
| `MAILBAG_LISTEN` | `127.0.0.1:8025` | Where to listen. |
| `MAILBAG_PER_ADDRESS_PER_DAY` | `5` | Reports taken from one client address in a day. |
| `MAILBAG_PER_DAY` | `100` | Reports taken in a day in all. Gmail stops an account after about 500 emails a day. |
| `MAILBAG_BLOCK` | None | Client addresses and ranges whose reports are refused, separated by commas or spaces, such as `203.0.113.7, 198.51.100.0/24`. Blocked reports don't count toward the limits. |

Each email's subject reads like `[Paperback 1.2.0] crash: The window closed`. Its Reply-To is the address the user gave, so replying answers them. The server never emails that address. It only checks in DNS that the address's domain takes email, to catch a typo while the user can still fix it. If the lookup fails, the report is taken anyway.

The limits are kept in memory, so restarting the server, such as to change `MAILBAG_BLOCK`, starts them over.

The server answers `202` when it sent the email, `400` for a malformed report, `403` for an app not in `MAILBAG_APPS` or a blocked address, `413` for a report that's too big, `422` when the reply address's domain doesn't take email, `429` when a limit is reached, and `502` when the email couldn't be sent.

### With Podman and Caddy

The `deploy` folder has what a Linux server needs. The server runs as your own user with rootless Podman, so only two steps need root.

1. Build the image: `podman build -f deploy/Containerfile -t mailbag-server .`. Compiling it takes more than 1 GB of memory. On a smaller server, build it on another machine, then copy it over with `podman save mailbag-server | ssh server podman load`.
2. Store the SMTP password as a Podman secret. This reads it without echoing it or saving it in your shell history:

   ```sh
   read -rsp 'App password: ' p && printf '%s' "$p" | podman secret create mailbag-smtp-password - && unset p
   ```

3. Copy `deploy/mailbag.env` to `~/.config/mailbag/mailbag.env` and fill it in.
4. Copy `deploy/mailbag.container` to `~/.config/containers/systemd/`, then run `systemctl --user daemon-reload` and `systemctl --user start mailbag`.
5. Keep the service running after you log out, and start it at boot: `sudo loginctl enable-linger $USER`.
6. Add the site in `deploy/Caddyfile` to the server's Caddyfile, with your domain, and reload Caddy with `sudo systemctl reload caddy`. Caddy replaces any `X-Forwarded-For` header a client sends, which the per-address limit relies on.

To change a setting, edit the environment file and run `systemctl --user restart mailbag`. To read the log, run `journalctl --user -u mailbag`.

## License

MIT
