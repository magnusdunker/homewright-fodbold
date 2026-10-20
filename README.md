# Fodbold til Homewright

En ekstern integration til [Homewright](https://github.com/magnusdunker/homewright), der viser, hvornår dine fodboldhold spiller næste gang, og mod hvem. Den kræver ingen nøgle eller konto.

## Installation

I Homewright: **Indstillinger → Integrationer → Fra GitHub**. Indsæt

```
https://github.com/magnusdunker/homewright-fodbold
```

og tryk **Installér**. Tilføj derefter **Fodbold** under **Tilføj integration**.

Vælg først de ligaer, dine hold spiller i, og derefter holdene. Du kan vælge hold fra flere ligaer på én gang, fx AGF og Liverpool. Holdene kan ændres senere under integrationens indstillinger.

Ligaerne er Superligaen, 1. division, Premier League, Championship, LaLiga, Bundesliga, 2. Bundesliga, Serie A, Ligue 1, Eredivisie, Primeira Liga, Scottish Premiership, Allsvenskan, Eliteserien og MLS. Under **Landshold** står Europas landshold.

## Hvad du får

En enhed pr. hold med to entities:

- **Næste kamp** (`sensor`, device class `timestamp`): hvornår kampen starter. Attributterne er:
  - `match`: hjemmeholdet først, fx "AGF - Viborg FF"
  - `when`: tidspunktet på dansk, fx "søndag 25. oktober kl. 18:00"
  - `opponent`, `home` og `home_away` (`hjemme` eller `ude`)
  - `competition` (fx "Superligaen" eller "Conference League") og `round` (fx "League Phase"), når det ikke er den almindelige sæson
  - `venue` og `city`
  - `tv`: kanalen, for 1. division
  - `time_confirmed`: falsk, når tidspunktet ikke er fastlagt endnu
  - `in_progress`: sand, mens kampen spilles
  - `upcoming`: de næste fem kampe med `start`, `when`, `match`, `opponent`, `home_away`, `competition`, `round`, `venue`, `tv` og `time_confirmed`
- **Modstander** (`sensor`): modstanderens navn, med `home_away`, `start`, `when`, `competition`, `venue` og modstanderens logo som billede.

Har holdet ingen kampe i programmet, fx i vinterpausen, er begge `unknown`.

Kampene hentes hver time. Fejler det tre gange i træk, bliver holdets entities utilgængelige, indtil det lykkes igen.

En påmindelse en time før kampen:

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

## Hvor kampene kommer fra

### ESPN

Integrationen læser ESPN's JSON på `https://site.api.espn.com/apis/site/v2/sports/soccer`. Det kræver ingen nøgle, og det dækker ligaer, pokalturneringer og de europæiske turneringer med tidspunkter, stadion og logoer. Det er ikke et officielt, dokumenteret API, så det kan ændre sig.

ESPN har et kampprogram pr. hold og turnering. Et holds næste kamp findes derfor ved at spørge dets liga, ligaens pokalturneringer og Champions League, Europa League og Conference League, og tage den første. For landshold spørges Nations League, VM, EM, kvalifikationerne og venskabskampe. ESPN har ikke DBU Pokalen, så den er ikke med for Superliga-holdene.

### DBU, for 1. division

ESPN har ikke 1. division, så de hold kommer fra DBU's resultatsider på [dbu.dk/resultater](https://www.dbu.dk/resultater/). DBU har intet åbent API, så integrationen læser de offentlige sider:

- Rækkesøgningen for landsdækkende seniorrækker for herrer i denne sæson. 1. division er rækken med `LIGA` i navnet (i dag "Betinia LIGA - Grundspil 2026/27"), så den findes også, når sponsoren skifter.
- Rækkens pulje og dens holdoversigt, når du vælger hold. Holdets side giver klubben.
- Klubbens side ved hver opdatering. Den viser de puljer, holdet spiller i i denne sæson, så ligaen, op- og nedrykningsspillet og Betano Pokalen kommer med, uden at integrationen kender deres id'er.
- Holdets kampprogram i hver af puljerne. Tiderne er dansk tid.

Hold- og klub-id'er er de samme fra sæson til sæson; puljernes er ikke. Ændrer DBU sine sider, kan læsningen gå i stykker.

## Udvikling

Bygget i Rust på SDK'en `thehome-plugin` fra [homewright-example-integration-rust](https://github.com/magnusdunker/homewright-example-integration-rust).

- `src/espn.rs`: ligaerne, holdene og kampprogrammet fra ESPN
- `src/dbu.rs`: 1. division fra DBU's resultatsider
- `src/lib.rs`: opsætningen, opdateringen og forbindelsen til Homewright
- `tests/`: tests mod rigtige svar fra ESPN og DBU i `tests/fixtures` (hentet 2026-10-20; DBU's sider uden kontaktpersoner), mod et falsk ESPN og DBU og med en falsk Homewright i `tests/host`

```
cargo test
cargo test -- --ignored live --nocapture       # mod ESPN
cargo test -- --ignored live_dbu --nocapture   # mod DBU
cargo build --release
```

En ny version udgives med et tag, fx `git tag v0.2.0 && git push origin v0.2.0`. Workflowet bygger til Windows, Linux (x86_64 og arm64, statisk musl) og macOS og lægger binærerne på releasen.

## Licens

MIT. Kampprogram og logoer er fra ESPN og DBU.
