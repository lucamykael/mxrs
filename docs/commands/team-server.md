# `mxrs team-server`

```sh
mxrs team-server login --pat-file FILE [--json]
mxrs team-server projects [--pat-file FILE]
mxrs team-server info APP_ID [--pat-file FILE]
mxrs team-server branches APP_ID [--pat-file FILE]
mxrs team-server commits APP_ID BRANCH [--pat-file FILE]
mxrs team-server clone APP_ID|URL TARGET [--branch NAME] [--depth N] [--pat-file FILE] [--json]
mxrs team-server status|fetch|pull [PATH] [--pat-file FILE] [--json]
mxrs team-server push [PATH] [--remote NAME] [--branch NAME] [--pat-file FILE]
```

mxrb's command, against the same services.

**Credentials.** `login` records only the path of a PAT file you keep, in
`~/.config/mxrs/credentials` (mode 0600); the PAT is never copied. A
command's `--pat-file`, or `MXRS_TEAM_SERVER_PAT_FILE`, is read before it.
The file is the PAT as plain text, JSON (`team_server_pat`), or a `.env`
with `MXRS_TEAM_SERVER_PAT=...`. Scopes: `mx:modelrepository:repo:read` to
read, `mx:modelrepository:repo:write` to push.

**APIs.** `info`, `branches` and `commits` ask Mendix's App Repository API
(`repository.api.mendix.com/v1`), `projects` every page of the Projects API,
each with `Authorization: MxToken <PAT>`, and print the answer as JSON. A
pagination link off the Projects API's host and paths is refused rather
than followed with the PAT.

**Git.** Only the official remote is taken: `https://git.api.mendix.com/<app
id>.git` (an app id alone means it). The PAT reaches Git only through a
`GIT_ASKPASS` helper written to a private temporary folder for the length of
one command, never in a URL, an argument or `.git/config`; without a PAT,
Git uses your own credential helper. `clone` refuses an existing
destination, removes what a failed clone left when it made no repository,
and checks every MPR at the repository's root as `mxrs validate` does;
`pull` (`--ff-only`) checks them again. `fetch` prunes. `push` pushes to the
remote named (`origin`) after checking it is Team Server's.
