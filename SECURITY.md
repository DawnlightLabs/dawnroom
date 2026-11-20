# Security

## Reporting a vulnerability

Please report security problems privately, not in a public issue.

Use GitHub's private vulnerability reporting: open the **Security** tab of
[DawnlightLabs/dawnroom](https://github.com/DawnlightLabs/dawnroom/security) and choose
**Report a vulnerability**.

Include what you found, how to reproduce it, the Dawnroom version (`dawnroom --version`) and your OS.
We'll acknowledge the report, keep you updated, and credit you in the fix if you'd like.

## Scope

Dawnroom runs on your own machine. The engine and the sd.cpp runtime it starts listen on `127.0.0.1` only.
In scope: the engine, CLI, MCP server, Studio, the installers and install scripts, and the model and runtime
download and verification paths. Weaknesses in third-party models or in sd.cpp itself are best reported
to their own projects (tell us too if Dawnroom ships the affected version).

## Supported versions

Only the latest release gets fixes.
