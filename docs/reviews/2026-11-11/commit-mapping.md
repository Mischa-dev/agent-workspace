# Commit mapping

Base: `dc695b5707ef6f74111f92b2e79ef2c884af1b75`. Reviewed code head: `3ac71534fcaf939aa986b184c45821f3ba5de27b`.

The first two local commits preserve the reviewed PR author. The final handoff commit contains documentation only. The external package records the final documentation/published head, avoiding a self-referential commit hash.

| Local code commit | Origin or purpose |
|---|---|
| `beb4bc671c575a35f2ace821225f32fbfe171dc7` | Cherry-pick of PR #211 `7cd01f8`; Claude authorship retained locally. |
| `93c87bdc8525404bb0f1f10d93d42ce7fcf7ab16` | Cherry-pick of PR #211 `9de9350ae4a95b2ee2a1c6258845ff463735f40c`; Claude authorship retained locally. |
| `7bc89d89a96d2b3541219902ca9dddfd2b068907` | Reject expired reply claims and bind completion context to retries |
| `56030eca7aea9d5bde2ba9f988b5ebc92f9f4520` | Refuse historical task-key ambiguity without a current-key fallback |
| `db7b3bd7994c68fbb8dc78b82430fed7936c1912` | Keep storage admission accurate across symlinks and concurrent removal |
| `8873d4c2c9414b974e5e6d555dd2c168430c7750` | Store the canonical validated public origin |
| `a4a867aedc09def9c35ceb6ac2f75f35cf42c13e` | Allow saved notes without a reminder time |
| `c7c5c17f912bc39c65369f81306d45fa4de91bd8` | Preserve message context and report partial mark-all-read failures |
| `3ac71534fcaf939aa986b184c45821f3ba5de27b` | Preserve readable moved originals while refusing ambiguous task keys |

## GitHub API commit identities

GitHub commit creation uses the connected account metadata, so its commit IDs differ from the local author-preserving series. Each code tree below was verified by its exact Git tree SHA. The portable bundle retains the local commits; the published branch has the identical reviewed contents. Commit messages also carry `Local-review-commit` trailers. The final documentation commit mapping is in the package manifest, because a file cannot contain its own final commit ID.

| Local code commit | GitHub commit | Identical tree |
|---|---|---|
| `beb4bc671c575a35f2ace821225f32fbfe171dc7` | `71711a1fde75e4e79802b0af17422a84eb9bd6de` | `b5e86e905cff687d6a48c29e6889afc37378c314` |
| `93c87bdc8525404bb0f1f10d93d42ce7fcf7ab16` | `a6fca487af2d2cd38a3e758d13f2c77472752a8f` | `ed82fee145fc2fb1c30926aca7844ebc503ee492` |
| `7bc89d89a96d2b3541219902ca9dddfd2b068907` | `3ef6dbfbf69f98e93097071fe76feef6bd49d8db` | `5d91172046d47ce79cf033326242ca473e6fa962` |
| `56030eca7aea9d5bde2ba9f988b5ebc92f9f4520` | `66fb7bb7396b5607aeedf0b06488d58475d0a5c9` | `3d4d573594f40d9dc25c71fab06f60e23a4fbc3e` |
| `db7b3bd7994c68fbb8dc78b82430fed7936c1912` | `beefd0b04d3f5963ed66194d7dead8376c8e20ee` | `56c46051e134dafff6fc19a41dcb5b0e98e6501b` |
| `8873d4c2c9414b974e5e6d555dd2c168430c7750` | `6cc9bf4b43f0ff852b95cf63e90587648760ca29` | `cc31ff163c90586f998266658fa610c410697468` |
| `a4a867aedc09def9c35ceb6ac2f75f35cf42c13e` | `5130243e458edc84c28c0abd3ccfc7f867181fcb` | `215d82a34884137f945948ec7b592f6e8390bac6` |
| `c7c5c17f912bc39c65369f81306d45fa4de91bd8` | `731f6f9234ef3220c03d0cbdf3480edf3888c960` | `958486fe8715c1520887a99839848d5381896633` |
| `3ac71534fcaf939aa986b184c45821f3ba5de27b` | `6a2dd0a7221a78e624a3b44893459d7396a5c68b` | `b7357e2c1443c8d9bc2098f8218581cf85e2018f` |
