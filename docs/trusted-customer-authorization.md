# Trusted customer authorization boundary

Linear: DEN-253

Every real customer request still presents a Supabase access token through the canonical host-only customer cookie or an explicit bearer header. `fiducia-customer` sends that credential directly to the configured `fiducia-auth` `/v1/me` endpoint. A successful session verification and non-empty organization membership are necessary but no longer sufficient.

The customer application now requires a structurally consistent version-1 authorization context produced by `fiducia-auth`:

- the `fiducia-customer` surface audience;
- the `customer:self-service` capability;
- only the known `admin`, `operator`, and `customer` roles;
- only the known admin/customer surface audiences and capabilities;
- no duplicate values;
- audiences and capabilities that exactly match the normalized role combination.

A context with no normalized roles is accepted only as the documented temporary legacy-customer shape: customer audience plus customer self-service capability and no admin grants. A privileged identity reaches both applications only when trusted metadata explicitly includes `customer` alongside `admin` or `operator`.

Raw `/v1/me.user.roles` strings are deserialized only for wire compatibility and are never consulted for authorization. A browser-supplied role header, admin cookie, admin-only audience, unknown future vocabulary, malformed response, or old auth response without the versioned context fails closed before `CustomerCtx` is created.

## Rollout dependency

Deploy the additive `fiducia-auth` producer PR before this consumer. During a mixed-version rollout, an old auth replica does not return `authorization`; this customer build rejects that response instead of silently falling back to raw roles. Use normal readiness and connection draining so traffic reaches compatible auth replicas.

## Deliberate follow-up

This PR establishes the receiving-surface gate. DEN-253 remains open for a route-by-route customer capability matrix, explicit dual-surface administration workflow, migration inventory for empty-role sessions, removal of that compatibility rule, and end-to-end negative tests across auth, customer, admin, edge, and proxies.
