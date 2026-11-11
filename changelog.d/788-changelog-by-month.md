### The changelog is a page of months; `scripts/changelog.sh since <date>` prints what is new (#788)
**What changed.** `CHANGELOG.md` no longer holds the entries: it is a short
page that links one file a month, `changelog/YYYY-MM.md`, newest first. The
entries up to 2026-11-11 are in `changelog/2026-11.md`, unchanged. An entry
waiting to be assembled is a file in `changelog.d/`.
**What to do.** After an engine pull, run
`bash engine/scripts/changelog.sh since <the day of your last pull>` instead
of reading `engine/CHANGELOG.md` top down. It needs bash and git, as the
pull does. A game's agent guide written by an older `flint init` still says
to read `CHANGELOG.md`; that page now says where to look.
