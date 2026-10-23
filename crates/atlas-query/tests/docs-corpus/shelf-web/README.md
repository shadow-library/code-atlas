# Shelf web

The browser client for shelves.

## Retries

`createShelfClient` builds the client; failed reads wait `backoffDelay()` before the next attempt.
See [the backoff helper](src/lib/backoff.ts).

## Labels

Labels go through `formatLabel`. The old `ShelfFetcher.load` helper is gone.
