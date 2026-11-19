# Relay transport and initial application V1

The version-aware delivery contract was introduced at backend `2faffd2`;
mandatory minimum enforcement is available at `9eb1319`. See the backend's
[MESSAGING_API.md](https://github.com/voidjack233/VOIDCHATS-2.0/blob/9eb1319/docs/MESSAGING_API.md)
and [VERSIONED_DELIVERY.md](https://github.com/voidjack233/VOIDCHATS-2.0/blob/9eb1319/docs/VERSIONED_DELIVERY.md).

The complete [initial V1 baseline](protocol-v1.md) includes text, receipts,
reply, reactions and message deletion in every normal Rust build.

The authenticated connection sends `protocol` with version `"1"` and waits for
matching `protocol_ok` before exposing its session to the storage worker.
Reconnect repeats the exchange. One bounded legacy in-flight delivery may be
buffered until negotiation completes. Malformed, unsolicited or mismatched
negotiation fails closed; negotiation also has a timeout.

All initial encrypted event types use `send`, `event`, correlated relay acceptance
and the existing ACK format. ACK follows successful atomic local processing or
safe durable deferral. Go need not inspect Signal content or change its database.
The Signal envelope/text/control codec's `v: 2` is independent of application
version `"1"`; receipt codec `v: 1` is also unchanged.

Canonical application versions are decimal JSON strings in `1..=2147483647`.
Generic `send_versioned` serialization and `event_versioned` parsing remain for
future incompatible contracts, but no initial V1 operation sends them. Transport
rejects an incoming minimum above the negotiated version before worker delivery;
Signal also checks the V1 receiver ceiling before pinning, duplicates or ratchet
writes. Requests above the session's version fail locally. This is a transport
guard, not peer capability discovery or Signal authorization.

A valid higher `update_required` minimum activates the blocking update interface
and ends automatic/manual reconnect for that worker. Network failures preserve
ordinary reconnect backoff. No cryptographic state is reset.

The relay retains its ordered queue and 30-day expiry policy. An unsupported
future/experimental head can block subsequent events until ACK or expiry;
metadata is never silently downgraded and expired controls cannot be claimed as
applied. No server capability probe or production change is introduced.
Loopback fixtures verify the contract against unchanged Go code.
