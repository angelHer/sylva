# Working on sylva

Only the things that reading the code will not tell you. The layering is
documented in `src/lib.rs`, the stack and the flags in `README.md`, and the test
style is plain to see in any test module — none of that is repeated here.

## This repository is not rustfmt-clean

`cargo fmt --check` fails on a clean checkout: 47 hunks across 16 files, from
rustfmt version drift rather than from anyone's choice. There is no
`rustfmt.toml`.

So **do not run `cargo fmt` across the tree while working on a change.** It will
reformat sixteen files you never touched, and a diff that mixes those with your
own work cannot be reviewed. Format only the files you are editing, or revert the
rest before committing.

Bringing the whole tree up to date is worth doing, but as its own `style:` commit
with nothing else in it.

## Comment prose is British English

`colour`, `behaviour`, `honours`, `canonicalise`, `recognise`, `centred`. The
American spellings that appear are all API names being quoted —
`fs::canonicalize`, `centered_and_justified` — and those stay as they are.

Comments here explain *why*, not what: the surrounding code shows what it does,
so a comment that restates it earns nothing. Match that density and that voice.

## Verify UI changes by rendering them

`--screenshot FILE` renders one frame and exits. Use it before claiming a visual
change works, and read the PNG back. It is not a curiosity; it has caught bugs no
unit test could see, including a header that read "repository" because
`file_name()` of `"."` is `None`, and buttons that rendered as empty boxes
because the bundled font has no glyph for `✕` or `⌂`.

`--cli` does the same for the Git layer without a compositor.
