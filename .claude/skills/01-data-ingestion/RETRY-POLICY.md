# Retry Policy

Every external fetch can fail. The policy is the same across sources,
with per-source backoff constants. Request **rates** are not here: the one
rate table is `SKILL.md` §5. This file holds only retry and backoff
constants.

## The Algorithm

    attempt = 1
    backoff = base_backoff
    while attempt <= max_attempts:
        response = fetch()
        if response.ok:
            return response
        if response.empty_series and source == EODHD:      # 200 with []
            mark_chunk_gone(reason = vendor_no_data)     # survivorship signal, not an error
            return gone
        if response.status == 404 and source == Yahoo:
            mark_chunk_gone(reason = vendor_no_data)     # survivorship signal, not an error
            return gone
        if response.status in (400, 401, 403, 404):
            log_fail(no_retry)
            return error
        if response.status == 429:
            sleep(rate_limit_backoff)
            attempt += 1
            continue
        if response.status >= 500:
            sleep(backoff)
            backoff *= 2
            attempt += 1
            continue
        if response.timeout:
            sleep(backoff)
            backoff *= 2
            attempt += 1
            continue
    log_fail(max_attempts_exceeded)
    return error

## Constants Per Source

| Source | Base backoff | Max attempts | Rate-limit (429) backoff |
|---|---|---|---|
| EODHD | 1s | 3 | 60s, then halve the refill rate for 10 minutes; a 401/403 (key or plan) stops the run |
| Yahoo | 2s | 3 | 60s, then halve the refill rate for 10 minutes |
| SEC EDGAR | 1s | 5 | 60s (a 403/429 from SEC may mean a temporary IP block: stop the source for the run) |
| FRED / ALFRED | 1s | 3 | 120s |
| iShares | 5s | 2 | 300s |

Bulk downloads (SEC zips) retry the interrupted `Range` request, up to 5
attempts, backoff 5s.

Backoff is exponential: 1s, 2s, 4s, 8s, 16s. Capped at 60s.

## Non-Retryable Failures

- HTTP 4xx (except 429). An EODHD `200` with `[]`, or a Yahoo `404` /
  `chart.error = Not Found`, is
  recorded as chunk status `gone` (reason `vendor_no_data`) and is **not** an
  ingestion failure: it feeds the survivorship coverage report
  (`SURVIVORSHIP.md`)
- Schema mismatch (the response is valid JSON but not the shape we expect)
- Empty response for a source that should always have data
- EDGAR filing that parses but is malformed XBRL (log, skip, alert)

## Retryable Failures

- HTTP 5xx
- HTTP 429
- Timeout
- Connection refused
- TLS handshake failure

## Escalation

When retries are exhausted:

1. Log `FAIL` in `data_quality` with the source, dataset, entity, chunk and
   attempt count. The chunk's manifest status becomes `failed` (resumable).
2. Append a `CRITICAL` row to the audit log (`append_durable`, `11-rust-event-bus` §5)
   once the audit writer exists (the execution phase). During the data
   collection phase the container log and `data_quality` are the record.
3. Exit the ingestion job non-zero.
4. The scheduler is responsible for alerting.

The system does not retry after a scheduled run completes. The next
scheduled run (or a backfill resume, `BACKFILL.md` §4) will attempt again;
only chunks not yet `compacted` or `gone` are retried. If a source has been failing for more
than 3 consecutive runs, the alert should escalate to manual review.

## Special Case: SEC Filings

SEC filings are the hardest to retry correctly.

- **Filing not yet available:** If a company was expected to file on
  2024-02-15 but the filing is not on EDGAR, retry in 1 hour, up to 3
  times that day. Then wait for the next scheduled run.
- **Filing available but XBRL malformed:** Log WARN, skip, retry in 4
  hours. Do not block other tickers.
- **Filing not yet accepted but filed:** SEC sometimes has a window
  between submission and acceptance. The `acceptance_datetime` is the
  authoritative timestamp. If missing, wait and retry.

## Logging

Every retry logs:

    [WARN] ingest_prices entity=AAPL chunk=full attempt=2 reason=timeout backoff=4s

Every exhausted retry logs:

    [FAIL] ingest_prices entity=AAPL chunk=full attempts=3 reason=timeout

These lines go to the container log and to `data_quality` (columns `entity`,
`chunk`, `attempts`).

## What Not to Do

- Do not retry on 4xx. The request is wrong; retrying makes it wrong
  again.
- Do not use a fixed backoff. Exponential prevents thundering herds.
- Do not retry forever. Bounded attempts, then escalate.
- Do not silently swallow failures. Every failed fetch is logged.
- Do not retry on a shorter schedule than the source's update cadence.
  EODHD publishes a day once, after the close. Retrying every 5 minutes wastes
  calls.