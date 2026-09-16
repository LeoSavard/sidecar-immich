# Edit in Sidecar — iOS Shortcut

iOS can't send shares to a web app, so a Shortcut reads the shared file and opens Sidecar in Safari.
Nothing is uploaded: the Shortcut sends the file's hash, name, capture date and size.

Create it in the Shortcuts app:

1. New Shortcut → **Details** → turn on **Show in Share Sheet**; **Receives**: Images and Files.
2. **Get Details of Images** → *Date Taken* (input: Shortcut Input).
3. **Format Date** → Date Format **ISO 8601**, **Include ISO 8601 Time** on. Rename the result `taken`.
4. **Get Details of Files** → *Name* (input: Shortcut Input) → **URL Encode** it. Rename `name`.
5. **Get Details of Files** → *File Size* → **Format File Size** → *Bytes*, units off. Rename `size`.
6. **Generate Hash** → *SHA1* of Shortcut Input. Rename `sha1`.
7. **URL** → `https://YOUR-SIDECAR-HOST/open?sha1=[sha1]&name=[name]&taken=[taken]&size=[size]`
8. **Open URLs**.

Name it **Edit in Sidecar**. In Immich, open a photo → Share → Edit in Sidecar.

Why ISO 8601: the Long date formats follow the phone's language and region settings, so a settings change would
silently break date matching. Sidecar reads the calendar date as written, in the phone's time zone.
