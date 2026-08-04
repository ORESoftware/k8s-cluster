# External probe location identity contract

The managed-public-beta availability SLO requires at least two genuinely
failure-independent observations. A `cell` identifies the Fiducia service cell
being measured; it does **not** identify where the observation originated.

## Trusted identity boundary

`probe_location` is a bounded monitoring-topology identity injected by the
trusted Prometheus scrape configuration. The probe process does not self-assert
this label. This prevents a compromised or misconfigured probe from claiming
that one runtime represents multiple independent locations.

The ingested cumulative source contract is:

```text
fiducia_external_probe_total{
  probe_location,
  cell,
  operation_class,
  result
}
```

The probe-generated textfile intentionally contains only `cell`,
`operation_class`, and `result`. Each reviewed scrape target adds exactly one
`probe_location` value with `honor_labels: false`. An incoming target must not
supply or override that label.

## Bounded values

`probe_location` must match:

```text
[a-z0-9][a-z0-9_-]{0,63}
```

It is an opaque operational ID such as `probe-a` or `probe-b`, not a hostname,
IP address, cloud account, customer name, GPS location, or unbounded provider
metadata.

## Independence evidence

Two distinct label values are necessary but not sufficient. DEN-1619 must record
and independently review that the corresponding locations do not share the same:

- physical host or Kubernetes cluster;
- scheduler/runtime or cumulative state authority;
- credential file or operator identity;
- outbound network/provider failure domain;
- ingress process or local reverse proxy;
- DNS resolver/failure path where practical.

The exact-candidate exporter requires the complete declared
`probe_location × cell × operation_class × result` matrix. A missing location,
an unexpected location, or duplicate series identity makes the evidence bundle
incomplete.

## Aggregation semantics

The public availability objective remains an aggregate ratio by `cell`; each
probe observation contributes equally to the numerator and denominator. Separate
per-location source-freshness, last-success, reset, duplicate-authority, and
location-count controls prevent that aggregation from hiding the loss or collapse
of an independent source.

A cell with fewer than two currently observed trusted `probe_location` values is
not eligible for managed-beta availability evidence, even when its aggregate
success ratio appears healthy.

## Prohibited labels and content

The new location label does not relax the low-cardinality boundary. Prometheus
must not receive organization, tenant, project, environment, resource key/path,
credential, endpoint URL, request/trace ID, response content, or raw error text.

## Evidence maturity

A merged contract and green rules/exporter tests are `specified` source evidence.
The source becomes `instrumented` only when two named external deployments emit
series carrying trusted scrape-injected location labels. It becomes `queryable`
only when the central rules, location-count alert, freshness views, and dashboard
are live. It becomes `measured` only after a completed exact-candidate window,
exported evidence, and independent review.
