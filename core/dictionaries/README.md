# Spell-check dictionaries

Hunspell dictionaries used by `grafium_core::spell`. They are compressed by
`core/build.rs` and embedded in the application, so spell checking works fully
offline. Each directory holds the unmodified upstream `index.aff` and
`index.dic` plus the upstream licence text (`LICENSE`).

| Code | Language | Source | Licence |
|------|----------|--------|---------|
| `en` | English (US) | [wooorm/dictionaries](https://github.com/wooorm/dictionaries) `dictionary-en` 4.0.0 (SCOWL `en_US`) | MIT AND BSD |
| `en-GB` | English (UK) | wooorm/dictionaries `dictionary-en-gb` 3.0.0 (SCOWL `en_GB-ise`) | MIT AND BSD |
| `es` | Spanish | wooorm/dictionaries `dictionary-es` 4.0.0 (RLA-ES) | GPL-3.0 OR LGPL-3.0 OR MPL-1.1 |
| `ru` | Russian | wooorm/dictionaries `dictionary-ru` 3.0.0 | BSD-3-Clause |
| `fr` | French | wooorm/dictionaries `dictionary-fr` 3.0.0 (Grammalecte) | MPL-2.0 |
| `hi` | Hindi | [LibreOffice/dictionaries](https://github.com/LibreOffice/dictionaries) `hi_IN` (GNU Aspell Hindi word list), commit `2bb5f4cd04` | GPL-2.0-or-later |
| `ar` | Arabic | LibreOffice/dictionaries `ar` (Ayaspell), commit `10e0418ad6` | GPL-2.0+ / LGPL-2.1+ / MPL-1.1+ |

`hi/LICENSE` was converted from Latin-1 to UTF-8 so that `©` stays readable.
The dictionary files themselves are byte-for-byte upstream.

All of these licences are compatible with Grafium's AGPL-3.0-only licence.

## Adding a language

1. Add a directory named after the language code with `index.aff`, `index.dic`
   and `LICENSE`.
2. Add one `language!(…)` line to the registry in `core/src/spell/mod.rs`.
3. Add a row above and a measured memory figure to the Spell check help.
