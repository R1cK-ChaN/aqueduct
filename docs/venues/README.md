# Venue facts

| Register | Id prefix |
| --- | --- |
| [Hyperliquid](hyperliquid.md) | `HL-` |
| [Binance](binance.md) | `BN-` |
| [Third-party data vendors](vendors.md) | `VN-` |

Each entry records one independently checkable statement the project relies
on. Venue pages group entries under **Rules**, **Feeds and archives** and
**Measured by us**, in that order. A measurement belongs here when a
decision or a value in code depends on it. The measurement note or report
keeps the method and full results.

## Entry format

| Field, in order | What to record |
| --- | --- |
| Id | Use the id alone as the heading. Start at 1 per file and assign each new entry the next number. Keep assigned ids and their anchors permanently. Never renumber or reuse one. |
| Statement | Write one to four sentences that could be shown false. Name the channel, market, sample or other condition on which the statement depends. |
| Kind | Use `rule` for how a venue behaves under its own rules. Attribute its published performance figures to the venue. Use `interface` for the content or encoding of a feed, endpoint or archive. Use `measured` for an observation we made. |
| Source | Link to the relevant public documentation. For an undocumented interface or rule evidenced in our samples or decoder tests, link to the repository evidence. For a measurement, link to a dated note in [docs/measurements/](../measurements/README.md) or to an existing standalone report that gives its method. Never use an operations runbook or a configuration file as a Source. |
| Checked | For a rule or interface, give the UTC date the source was checked. For a measurement, give the sample date and extent, including the site when it matters. Identify missing sample details explicitly. |
| Status | Use `confirmed` when the source supports the statement, `unverified` when it is unavailable, silent or unknown, and `disputed` when it says something different. For either of the last two, add one sentence describing what the check found. The date on such an entry records the check attempt. |

## Citing and updating an entry

Use the id as the link text. For example, an RFC in `docs/rfc/` cites
`[HL-12](../venues/hyperliquid.md#hl-12)`. A simulator approximation cites
the fact it approximates, and the RFC owns the choice of approximation.

When a venue changes, update the entry and its Checked date first, then
update every document that cites that id. Follow the same sequence when a
new measurement replaces a sample used in a decision.
