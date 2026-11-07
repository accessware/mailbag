# mailbag

Problem reports from desktop apps, emailed to the developer. The app sends a report to a small server you run, and the server emails it to you. The app never holds a mail password, and the server only ever emails one address, so it's no use to a spammer.

| Crate | What it does |
| --- | --- |
| `mailbag` | The report: what happened, the app and system it happened on, and the logs. Sends it to a server. |
| `mailbag-server` | Takes reports over HTTP and emails each one, with its logs attached. |

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
