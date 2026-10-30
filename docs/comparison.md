# How BudgetLaser compares

Checked on 2026-10-30 from public product pages and retailers. Prices and features change; if
something here is out of date, please open an issue or a pull request.

| Product | What it is | Lights up a projector | Patchable DMX fixture | Plays ILDA | Open source | Source |
|---|---|---|---|---|---|---|
| **BudgetLaser** | Projector laser-effect fixture | Yes | Yes (13/14/17/18ch) | Yes, from DMX | Yes (MIT) | this repo |
| Liberation | Laser control and 3D preview | No, previews on a monitor | Takes DMX in | Outputs to laser DACs | No | [liberationlaser.com](https://liberationlaser.com) |
| Pangolin Beyond / QuickShow | Laser control and preview | No | Takes DMX in | Outputs to Pangolin hardware | No | [pangolin.com](https://pangolin.com/en-kr/products/fb3qs-with-quickshow) |
| Capture | Lighting pre-visualisation | No | Visualises fixtures | Laser feed in the preview | No | [capture.se](https://www.capture.se/Products/Capture/Pricing) |
| QLC+ 3D view | Free lighting controller with 3D preview | No | Yes (it is the controller) | No laser rendering found | Yes | [github.com/mcallegari/qlcplus](https://github.com/mcallegari/qlcplus/) |
| MusicBeam | Projector beam show | Yes | No, audio-driven | No | Yes (MIT) | [musicbeam.org](https://www.musicbeam.org/) |
| Resolume / MadMapper / TouchDesigner | VJ and media software | Yes, as content you build | DIY | MadMapper's MadLaser outputs to real lasers | No | [resolume.com](https://resolume.com/article/27), [madmapper.com](https://www.madmapper.com/madbundle) |
| laser-dac-rs idn-simulator | Developer tool | No, draws in a window | No | Takes IDN in | Yes (MIT) | [github.com/ModulaserApp/laser-dac-rs](https://github.com/ModulaserApp/laser-dac-rs) |
| Entry-level DMX laser (e.g. Cameo Wookie 400 RGB) | Real Class 3B laser | n/a | Yes, 3 or 9 channels, preset patterns | Not on this class ([example](https://thomann.fr/laserworld_el_400rgb_s.htm)) | n/a | [thomann.fr](https://www.thomann.fr/cameo_wookie_400_rgb_animation_laser.htm) |

What we did not find elsewhere: a tool that turns a projector into the effect, patches as a DMX
fixture **and** plays ILDA files from the desk.

## Why not just use a laser?

Real show lasers are the brighter, sharper option, and BudgetLaser does not replace a certified
show laser. They come with more paperwork:

- In the US, Class 3B and 4 light shows need an FDA/CDRH variance, and audience scanning needs
  specific approval ([FDA Laser Notice 55](https://www.fda.gov/regulatory-information/search-fda-guidance-documents/procedures-renewal-and-amendment-certain-laser-light-show-variances-laser-notice-55)).
- In the UK, HSE guidance expects competent operators, a risk assessment and no-go zones for the
  audience ([INDG224](https://www.hse.gov.uk/pubns/indg224.htm)).
- Venues and permits often ask for large liability cover, for example $5M in Henderson, Nevada
  ([permit](https://cityofhenderson.com/home/showpublisheddocument/11622/638248384938830000)).
- Playing your own ILDA animations on a real laser needs a DAC and laser software, for example an
  Ether Dream 4 ([$229-289](https://www.nlfxpro.com/ether-dream-4)) plus a show program.

A projector is not exempt from rules either: see [Safety](../README.md#safety).
