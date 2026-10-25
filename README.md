# Fodbold for HomewrightHQ

An external integration for [HomewrightHQ](https://github.com/magnusdunker/homewrighthq) that shows when your football teams play next, and against whom. It needs no key or account.

## Installation

In HomewrightHQ: **Settings → Integrations → From GitHub**. Paste

```
https://github.com/magnusdunker/homewrighthq-fodbold
```

and press **Install**. Then add **Fodbold** under **Add integration**.

First choose the leagues your teams play in, then the teams. You can choose teams from several leagues at once, e.g. AGF and Liverpool. The teams can be changed later under the integration's options.

The leagues are Superligaen, 1. division, Premier League, Championship, LaLiga, Bundesliga, 2. Bundesliga, Serie A, Ligue 1, Eredivisie, Primeira Liga, Scottish Premiership, Allsvenskan, Eliteserien and MLS. Under **Landshold** (national teams) are Europe's national teams.

## What you get

A device per team with two entities:

- **Næste kamp** (next match, `sensor`, device class `timestamp`): when the match starts. The attributes are:
  - `match`: home team first, e.g. "AGF - Viborg FF"
  - `when`: the time in Danish, e.g. "søndag 25. oktober kl. 18:00"
  - `opponent`, `home` and `home_away` (`hjemme` or `ude`, home or away)
  - `competition` (e.g. "Superligaen" or "Conference League") and `round` (e.g. "League Phase"), when it is not the regular season
  - `venue` and `city`
  - `tv`: the channel, for 1. division
  - `time_confirmed`: false when the time is not fixed yet
  - `in_progress`: true while the match is being played
  - `upcoming`: the next five matches with `start`, `when`, `match`, `opponent`, `home_away`, `competition`, `round`, `venue`, `tv` and `time_confirmed`
- **Modstander** (opponent, `sensor`): the opponent's name, with `home_away`, `start`, `when`, `competition`, `venue` and the opponent's logo as the picture.

If the team has no matches in the schedule, e.g. in the winter break, both are `unknown`.

The matches are fetched every hour. If that fails three times in a row, the team's entities become unavailable until it succeeds again.

A reminder an hour before the match:

```yaml
triggers:
  - trigger: time
    at:
      entity_id: sensor.agf_naeste_kamp
      offset: "-01:00:00"
actions:
  - action: notify.notify
    data:
      message: >
        {{ state_attr('sensor.agf_naeste_kamp', 'match') }} starter om en time
        ({{ state_attr('sensor.agf_naeste_kamp', 'competition') }}).
```

## Where the matches come from

### ESPN

The integration reads ESPN's JSON at `https://site.api.espn.com/apis/site/v2/sports/soccer`. It needs no key, and it covers leagues, cup competitions and the European competitions with kick-off times, stadium and logos. It is not an official, documented API, so it can change.

ESPN has a schedule per team and competition. A team's next match is therefore found by asking its league, the league's cup competitions and the Champions League, Europa League and Conference League, and taking the first one. For national teams it asks the Nations League, World Cup, European Championship, the qualifiers and friendlies. ESPN does not have DBU Pokalen, so it is not included for the Superliga teams.

### DBU, for 1. division

ESPN does not have 1. division, so those teams come from DBU's results pages at [dbu.dk/resultater](https://www.dbu.dk/resultater/). DBU has no open API, so the integration reads the public pages:

- The league search for nationwide men's senior leagues this season. 1. division is the league with `LIGA` in its name (currently "Betinia LIGA - Grundspil 2026/27"), so it is still found when the sponsor changes.
- The league's group and its team list, when you choose teams. The team's page gives the club.
- The club's page at each update. It shows the groups the team plays in this season, so the league, the promotion and relegation play-offs and Betano Pokalen are included without the integration knowing their ids.
- The team's schedule in each of the groups. The times are Danish time.

Team and club ids are the same from season to season; group ids are not. If DBU changes its pages, the reading can break.

## Development

Built in Rust on the `thehome-plugin` SDK from [homewright-example-integration-rust](https://github.com/magnusdunker/homewrighthq-example-integration-rust).

- `src/espn.rs`: the leagues, the teams and the schedule from ESPN
- `src/dbu.rs`: 1. division from DBU's results pages
- `src/lib.rs`: the setup, the update and the connection to HomewrightHQ
- `tests/`: tests against real responses from ESPN and DBU in `tests/fixtures` (fetched 2026-10-20; DBU's pages without contact persons), against a fake ESPN and DBU and with a fake HomewrightHQ in `tests/host`

```
cargo test
cargo test -- --ignored live --nocapture       # against ESPN
cargo test -- --ignored live_dbu --nocapture   # against DBU
cargo build --release
```

A new version is released with a tag, e.g. `git tag v0.2.0 && git push origin v0.2.0`. The workflow builds for Windows, Linux (x86_64 and arm64, static musl) and macOS and puts the binaries on the release.

## License

MIT. Schedule and logos are from ESPN and DBU.
