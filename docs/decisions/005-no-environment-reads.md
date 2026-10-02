# 005 — No environment reads

## What is decided

Lakelands reads no environment variables. Everything that configures a
lake (the engine and its tuning, limits, the optional key handle) arrives
through `Location`; everything that configures a server arrives through
server configuration. A host that keeps its settings in the environment
reads them itself and passes them in.

## Why

A library that reads the environment has hidden inputs. Two lakes in one
process cannot be configured differently, a test cannot set a value
without leaking it into the next test, and a host cannot see from the
call site what the store will do. Passing configuration explicitly makes
it visible, per lake, and testable.

It also keeps the store's configuration surface a documented contract:
every knob is a field someone can read in `Location` or the server
configuration, not a variable name found by searching the source.

## What it is not

- It is not a rule about where an operator keeps settings. The CLI and
  the server turn what the operator gives them into server
  configuration and `Location`; the store below them sees only those.
- It does not forbid a host from using the environment. The rule binds
  Lakelands' code, not the programs that embed it.
- Configuration is not a seam. Seams are for behaviour that needs the
  outside world; settings are data passed at the call site.
