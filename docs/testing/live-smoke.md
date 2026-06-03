# Live Smoke Checklist

This checklist is the manual live-account gate layered on top of the offline test suite. Run it only against a controlled `My Drive` account.

## Preconditions

- `make test-all`
- `make test-coverage`
- `make build-release`
- Google Desktop OAuth credentials configured locally
- test account contains at least:
  - one image with Drive `imageMediaMetadata`
  - one intentionally shared file you can safely unshare and then restore if needed
- one disposable file you can safely move to trash and restore if needed
- a private visible My Drive folder for remote DB sync smoke testing

## Checklist

- [ ] `auth login`
- [ ] `auth status`
- [ ] first `sync`
- [ ] second `sync` (delta/no-op sanity)
- [ ] `report all`
- [ ] `find shared --shared`
- [ ] `inspect file <known-id>`
- [ ] `inspect exif <known-image-id>`
- [ ] `unshare --shared-with anyone` dry run
- [ ] one controlled `unshare --apply --yes`
- [ ] `trash --path <disposable-file-path>` dry run
- [ ] optional one controlled `trash --path <disposable-file-path> --apply --yes`
- [ ] `db remote status`
- [ ] one controlled `db remote push --yes` into a private folder
- [ ] `db remote status` confirms manifest checksum and remote DB metadata
- [ ] post-apply `sync --full`
- [ ] verification query showing the intended permission is gone and any trashed disposable file is absent from the local snapshot
- [ ] `auth logout`

## Recording guidance

Capture at least:

- the account email used for the run
- the database path/profile directory
- the file ID used for the controlled unshare apply
- the file ID/path used for any controlled trash apply
- the remote DB folder ID/name and manifest checksum
- whether any scope upgrades were requested and accepted
- any unexpected prompts, errors, or retry behavior

## Scope reminder

This smoke checklist is for `My Drive` only. Shared Drives remain deferred and should not be treated as part of release readiness.

Remote DB smoke tests must use a private folder. If `SECURITY ALERT` appears, stop the smoke run and remove sharing from the folder, DB file, and manifest file before retrying.
