# Security policy

BudgetLaser listens on a show network and turns what it hears there into output
on a projector. It is software and adds no light of its own, so there is no
laser-safety question here: every beam is the projector's. What there is, is a
process that parses packets and files arriving from the network and puts the
result on screen during someone's show.

## Reporting a vulnerability

Use GitHub's **private vulnerability reporting** on this repository
(Security › Report a vulnerability). Please do not open a public issue or pull
request for a security problem.

Useful to include: the version you ran, the platform, which input was in use
(Art-Net, sACN, the Enttec adapter, Ether Dream or IDN), and the smallest
packet, file or configuration that shows the problem.

This is a one-person project. Reports are read as soon as there is time, with
no promised response time. A confirmed fix goes into the next release, and you
are credited unless you prefer otherwise.

## Supported versions

BudgetLaser is pre-1.0. Only the latest release gets fixes; older ones are not
patched, so update before reporting if you can.

## Scope and known limits

The following is how it is meant to work, not a vulnerability.

| What | Where it listens | Authentication |
|---|---|---|
| Ether Dream DAC emulation | TCP 7765 on all interfaces, plus a UDP broadcast on 7654 | none |
| IDN | UDP 7255 on all interfaces | none |
| Art-Net in | UDP 6454 | none |
| sACN in | UDP 5568 | none |
| Enttec DMX USB Pro | USB serial | local access |

None of those protocols authenticate anything; that is true of the protocols
themselves, not a choice made here. Anyone who can reach the machine on that
network can take the output over, which is the same exposure every other
fixture on a lighting network has. Keep the show network off the public one.

What is worth a report: input that crashes, hangs or wedges the process, or
that reaches past the renderer. A malformed ILDA file, a truncated Ether Dream
or IDN packet and a DMX universe full of nonsense are all things this should
survive, and a crash on any of them is a bug. Being able to drive the output
from the network is not.
