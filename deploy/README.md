# Plain Web на своём сервере

От «есть VPS и домен» до «открыла с телефона». Ubuntu 24.04 (или другой Linux x64 с systemd), Caddy перед Plain выдаёт сертификат. Везде ниже `notes.example.com` — ваше имя. Все команды — от root на сервере, если не сказано иное.

## Первый раз

1. **DNS.** A-запись имени на адрес сервера. Проверка: `dig +short notes.example.com` отвечает адресом.
2. **Пользователь и папки.**
   ```bash
   useradd --system --home /var/lib/plain-web --shell /usr/sbin/nologin plain
   install -d -o plain -g plain -m 700 /srv/notes /var/lib/plain-web
   install -d -m 755 /opt/plain-web
   ```
3. **Программа.** Последний релиз `web-*` (без `-dr`) — на странице Releases. `update.sh` берётся из архива; дальше он обновляет сам себя вместе с остальным.
   ```bash
   cd /tmp && T=web-0.12 && N=plain-web-$T-linux-x64
   curl -fLO https://github.com/ekaterina-ya/plain/releases/download/$T/$N.tar.gz
   curl -fLO https://github.com/ekaterina-ya/plain/releases/download/$T/$N.tar.gz.sha256
   sha256sum -c $N.tar.gz.sha256 && tar xzf $N.tar.gz && $N/deploy/update.sh $T
   ```
4. **Вход** — пароль и QR для приложения-аутентификатора. Запускать в своём терминале (нужен ввод):
   ```bash
   ssh -t root@сервер 'sudo -u plain /opt/plain-web/current/plain-web init --data /var/lib/plain-web'
   ```
5. **Служба.** В `plain-web.service` — своё имя в `--host`:
   ```bash
   cp /opt/plain-web/current/deploy/plain-web.service /etc/systemd/system/
   nano /etc/systemd/system/plain-web.service
   systemctl daemon-reload && systemctl enable --now plain-web
   curl -fsS http://127.0.0.1:8642/robots.txt
   ```
6. **Caddy** (`apt install caddy`, если его ещё нет) — блок из `deploy/caddy.txt` со своим именем в `/etc/caddy/Caddyfile`, затем `systemctl reload caddy`. Порты 80 и 443 должны быть открыты.
7. Открыть `https://notes.example.com` на компьютере и телефоне.

**Caddy в Docker.** Контейнер достаёт до хоста по шлюзу своей сети (`docker network inspect …` → Gateway, например `172.17.0.1`). Тогда в службе `--listen <шлюз>:8642` и `After=docker.service`, в блоке Caddy — `reverse_proxy <шлюз>:8642`, и если включён ufw — `ufw allow from <подсеть сети> to <шлюз> port 8642 proto tcp`.

## Обновление

Метка `web-*` в репозитории собирает релиз (GitHub Actions, `.github/workflows/web-release.yml`). На сервере:

```bash
/opt/plain-web/current/deploy/update.sh web-0.5
```

Откат — та же команда с прошлой меткой: папка релиза на месте, скачивать ничего не нужно. Старые релизы копятся в `/opt/plain-web/releases` — лишние можно удалить руками.

Если в релизе изменился `deploy/plain-web.service`, его надо скопировать заново: `cp /opt/plain-web/current/deploy/plain-web.service /etc/systemd/system/ && systemctl daemon-reload && systemctl restart plain-web`.

## Обслуживание

- **Свои файлы в библиотеку.** Скопированное под root принадлежит root, и Plain не сможет его сохранять. После копирования: `chown -R plain:plain /srv/notes`.
- **Новый пароль.** Сначала остановить службу — иначе команда откажется, потому что сервер держит старые данные в памяти:
  ```bash
  systemctl stop plain-web
  sudo -u plain /opt/plain-web/current/plain-web reset-password --data /var/lib/plain-web
  systemctl start plain-web
  ```
  Потерян телефон с аутентификатором — то же самое с `init` вместо `reset-password`.
- **Журнал.** `journalctl -u plain-web` — запуск, входы и неудачные попытки.
- **Ключи Syncthing** (`/var/lib/plain-syncthing`) в бэкап не входят: потерялись — сопрячь Mac заново, это минута.
- **Резервные копии.** Копировать обе папки — `/srv/notes` (заметки) и `/var/lib/plain-web` (вход, сессии, история, корзина).

## Mac: десктопный Plain и синхронизация

Основной редактор на Mac — десктопный Plain; его папка `~/Notes` и `/srv/notes` на сервере — одна папка через Syncthing (напрямую между устройствами, по зашифрованному каналу, только сопряжённые устройства).

**Сервер** (один раз):

1. Syncthing 2.x из официального репозитория:
   ```bash
   curl -fsSL https://syncthing.net/release-key.gpg -o /etc/apt/keyrings/syncthing-archive-keyring.gpg
   echo "deb [signed-by=/etc/apt/keyrings/syncthing-archive-keyring.gpg] https://apt.syncthing.net/ syncthing stable-v2" > /etc/apt/sources.list.d/syncthing.list
   apt update && apt install syncthing
   ```
2. Служба и порт:
   ```bash
   cp /opt/plain-web/current/deploy/plain-syncthing.service /etc/systemd/system/
   systemctl daemon-reload && systemctl enable --now plain-syncthing
   ufw allow 22000/tcp comment 'plain syncthing' && ufw allow 22000/udp comment 'plain syncthing'
   install -o plain -g plain -m 600 /opt/plain-web/current/deploy/stignore /srv/notes/.stignore
   ```
3. Настройка (`ST="sudo -u plain syncthing cli --home=/var/lib/plain-syncthing"`): без поиска в локальной сети и UPnP (`$ST config options local-ann-enabled set false`, `$ST config options natenabled set false`), пароль панели (`$ST config gui user set …`, `$ST config gui password set …`); устройство Mac (`$ST config devices add --device-id <ID Mac>`); папка `plain-notes` → `/srv/notes`, общая с Mac, наблюдение с задержкой 1 с и корзина на 30 дней (`/srv/notes/.stversions` — удалённое или перезаписанное с Mac можно достать оттуда).
4. ID сервера — `sudo -u plain syncthing device-id --home=/var/lib/plain-syncthing`.

**Mac:** `brew install --cask syncthing-app` → значок в строке меню → Open. Создать пустую `~/Notes` и положить в неё `deploy/stignore` как `.stignore` — до принятия папки. Добавить сервер по его ID с адресом `tcp://<адрес сервера>:22000, quic://<адрес сервера>:22000`; принять папку `plain-notes` с путём `~/Notes`, в её настройках «Наблюдение за изменениями» — задержка 1 с. Папку `~/Sync`, которую приложение создаёт само, можно удалить.

**Десктопный Plain:** релиз оригинала или сборка из форка (`.github/workflows/macos.yml`, запуск вручную, `.dmg` — в артефактах сборки). Подписи Apple нет: если macOS не даёт открыть — Системные настройки → Конфиденциальность и безопасность → «Открыть всё равно».

**Что знать:**
- Изменения доходят за пару секунд в обе стороны. Оба Plain видят изменения на диске и не перезаписывают несохранённое.
- Один файл правили одновременно в двух местах без сети — рядом появится `заметка.sync-conflict-….md`; выбрать нужное и лишнее удалить.
- На Mac имена без учёта регистра: `Note.md` и `note.md` в одной папке Mac принять не сможет (Syncthing скажет о конфликте регистра) — переименовать один.
