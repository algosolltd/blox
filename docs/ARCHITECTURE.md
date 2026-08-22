# Architecture

`blox-core` is one deep in-process module around one order book. `blox-server`
is a transport adapter which owns a registry of those books and serializes all
mutations through one engine thread.

Inside `blox-server`, the command execution module owns registry semantics and
turns each protocol command into one correlated reply plus optional book
events. The TCP adapter owns framing, subscriptions, bounded queues and
connection shutdown. This seam keeps command behavior directly testable
without sockets while process tests verify the adapter end to end.

Provider normalization, cross-provider aggregation, synthetic markets,
conditional orders, sizing and broker routing deliberately do not live here.
