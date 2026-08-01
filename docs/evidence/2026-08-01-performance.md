# Worker and Renderer Performance Evidence

Date: 2026-08-01

## Environment

- macOS 26.5.2 on arm64
- Node.js 22.21.1
- npm 10.9.4
- Herdr Math commit: `e90507c`
- Command: `npm run test:performance`

The command ran in three independent Vitest processes. Each process executed one fake-socket worker lifecycle,
five maximum-size boundary resolutions, four real browser renders, three invalid renders, and three forced timeout
renders. The full automated test suite then ran the same gate concurrently with the other test files.

## Regression budgets

| Measurement | Budget |
|---|---:|
| Maximum-size boundary resolution | < 1,000 ms |
| Fake-socket harness startup | < 1,000 ms |
| Working or completion worker event | < 1,000 ms |
| Cold render | < 4,000 ms |
| Warm render median | < 2,000 ms |
| Representative PNG | < 128 KiB |
| Node RSS | < 1,024 MiB |
| Node RSS growth during renders | < 256 MiB |

These regression budgets are intentionally above the observed values but below the 8-second renderer safety
timeout where applicable. Product policy still independently rejects PNG output above 512 KiB.

## Dedicated-process results

| Measurement | Median | Maximum |
|---|---:|---:|
| Idle test-process Node RSS | 162.7 MiB | 163.4 MiB |
| Fake-socket harness startup | 1.3 ms | 1.4 ms |
| Working event | 23.7 ms | 28.4 ms |
| Completion event with static renderer | 27.1 ms | 28.0 ms |
| Maximum-size boundary resolution | 16.7 ms | 16.9 ms |
| Cold real render | 173.5 ms | 373.0 ms |
| Warm real render median | 149.4 ms | 163.7 ms |
| Node RSS after real renders | 184.4 MiB | 185.6 MiB |
| Node RSS growth during real renders | 21.8 MiB | 22.2 MiB |
| Representative PNG | 2,098 bytes | 2,098 bytes |

The representative image was 127 by 104 pixels. All twelve successful renders across the three processes had
the same decoded-pixel SHA-256 value. All nine invalid runs rejected before browser startup. All nine timeout
runs closed their backend. Every real browser backend reported no owned page, context, or browser after return.

The full 253-test run also passed without retry. Under parallel suite load, the performance case observed a
107.3 ms completion event, 452.6 ms cold render, 233.8 ms warm median, and 22.0 MiB Node RSS growth.

## Interpretation and limits

Herdr Math has no resident controller: startup and event hooks are one-shot processes, so the installed plugin
does not intentionally own an idle process. The idle RSS above is the Vitest process before the measured work;
it is not a claim about a real Herdr session. Real-process idle and terminal runtime evidence belongs to Phase 8.

Node RSS does not include the browser subprocess tree. The renderer selection experiment separately recorded an
approximate 482 MiB aggregate browser-path snapshot, with the limitation that it was not a synchronized
cross-platform peak. See [renderer candidate evidence](2026-08-01-renderer-candidates.md).

## Acceptance evidence

- AT-208: Five maximum-size repeated-anchor resolutions per process stayed within the bounded candidate and
  1,000 ms regression budget.
- AT-404: Three forced timeouts per process returned `renderer_timeout`, closed the backend, and did not prevent
  subsequent measured work.
- AT-409: Repeated success, invalid, and timeout cases left every owned renderer resource closed.
- AT-410: Current latency, memory, output size, and cleanup measurements supplement the recorded browser versus
  browser-free selection evidence.
