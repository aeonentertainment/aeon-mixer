# 0025: Each listed device says its transport, so a fallback can pass over a virtual one

**Status:** proposed, 2026-11-13. The mapping is from documentation and driver sources; no device
has been listed with it yet. The checks a person owes are at the end.

## The problem

When the default output will not start, a host can open the first other output in
`outputDevices()` that does (section 5). On a Mac that list holds every device with an output
stream, in the OS's order, and a virtual device is in it: BlackHole, ZoomAudioDevice, Microsoft
Teams Audio. It starts and plays to nobody, so the room goes silent while the host's log says it
is playing. `{ id, name, isDefault }` cannot tell it from a speaker, and matching names breaks on
the next driver anyone installs.

## Decision

Every entry of `outputDevices()` and `inputDevices()` carries `transport`, one of nine values. Each
is there because a fallback ranks it differently from the others.

| `transport` | What it is                                                                | What a fallback can do with it                                                   |
| ----------- | ------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `builtIn`   | the machine's own codec: a laptop's speakers, a desktop's line out        | take it                                                                          |
| `usb`       | a USB interface, headset or speaker                                       | take it                                                                          |
| `other`     | hardware on a bus not named here: a PCIe card, Thunderbolt, FireWire, AVB | take it                                                                          |
| `display`   | HDMI or DisplayPort to a monitor, TV or receiver                          | take it after the three above: it may have no speakers, and no OS says (below)   |
| `bluetooth` | Bluetooth classic or LE                                                   | take it last of the hardware: measured outputs run 173 ms to 308 ms late (below) |
| `aggregate` | a macOS aggregate or multi-output device a person built from real ones    | take it only after the hardware: it plays, through members the host cannot see   |
| `network`   | AirPlay, a remote desktop session's audio                                 | never unprompted: it plays in another place, about 2 s late                      |
| `virtual`   | a driver with no hardware behind it; its sound goes to other software     | never unprompted                                                                 |
| `unknown`   | the OS or driver did not say, or reading it failed                        | never unprompted: it may be virtual                                              |

The engine itself does not rank: its follower plays on the pinned output or the default (decision
0016), and the host owns the fallback. A device the person chose, by default or by pin, is theirs
whatever its transport.

The union is closed and complete now, because adding a value later breaks a host's exhaustive
`switch`. A transport an OS adds later maps into `other`, `network` or `unknown`.

### Against the issue's seven

#165 proposed `builtIn | usb | bluetooth | display | virtual | aggregate | other`. Two values are
added, and nothing is removed:

- **`network`**, because CoreAudio gives AirPlay a transport of its own, and a host that ranked it
  as `other` would send the room's mix about two seconds late. Shairport Sync, an AirPlay 2
  receiver, puts the delay of a realtime stream at "usually around two seconds"
  ([AIRPLAY2.md](https://github.com/mikebrady/shairport-sync/blob/master/AIRPLAY2.md)). That is
  67 to 100 times the 20 to 30 ms latency budget.
- **`unknown`**, apart from `other`, because `other` says the device is hardware and `unknown` does
  not. CoreAudio has an `Unknown` (0) transport, Windows' heuristic an `Unknown` bus, and a driver
  may not answer at all. A host that folded those into `other` would rank a device that may be
  virtual with a PCIe card.

`aggregate` stays apart from `virtual` because an aggregate plays to real devices: Audio MIDI
Setup builds it from them. Folding it into `virtual` would hide a working output.

## How each OS maps

The rule on each OS reads in order; the first that matches wins.

### macOS: `kAudioDevicePropertyTransportType`

[`kAudioDevicePropertyTransportType`](https://developer.apple.com/documentation/coreaudio/kaudiodevicepropertytransporttype)
on the device, global scope. The constants are in `objc2-core-audio` 0.3.2, which `devices`
already uses (`src/generated/AudioHardware.rs`).

| CoreAudio constant                                            | Code                                                   | `transport` |
| ------------------------------------------------------------- | ------------------------------------------------------ | ----------- |
| `BuiltIn`                                                     | `bltn`                                                 | `builtIn`   |
| `USB`                                                         | `usb `                                                 | `usb`       |
| `Bluetooth`, `BluetoothLE`                                    | `blue`, `blea`                                         | `bluetooth` |
| `HDMI`, `DisplayPort`                                         | `hdmi`, `dprt`                                         | `display`   |
| `AirPlay`                                                     | `airp`                                                 | `network`   |
| `Aggregate`                                                   | `grup`                                                 | `aggregate` |
| `Virtual`                                                     | `virt`                                                 | `virtual`   |
| `PCI`, `FireWire`, `Thunderbolt`, `AVB`, `ContinuityCapture*` | `pci `, `1394`, `thun`, `eavb`, `ccap`, `ccwd`, `ccwl` | `other`     |
| `Unknown`, any other value, or a read that fails              | `0`                                                    | `unknown`   |

What the virtual devices report:

- **BlackHole** returns `kAudioDeviceTransportTypeVirtual` for the device and its box
  ([BlackHole.c:2552](https://github.com/ExistentialAudio/BlackHole/blob/6008d295ff00c8b57412a23564619963f4ecbeac/BlackHole/BlackHole.c#L2552),
  and line 1970 for the box).
- **ZoomAudioDevice** and **Microsoft Teams Audio** both show `Transport: Virtual` in one person's
  `system_profiler SPAudioDataType` output, posted on Zoom's forum on 2023-10-06
  ([thread](https://community.zoom.com/meetings-2/weird-audio-issue-41697)). `system_profiler`
  reads the same property. Neither driver's source is public, so this is one machine's word for it.

The transport is whatever the driver says. BlackHole's author offered to report USB instead,
because some apps refuse a virtual output, and a user who rebuilt it that way confirmed it works
([discussion #770](https://github.com/ExistentialAudio/BlackHole/discussions/770)). A driver that
claims USB is listed as `usb`. The engine does not second-guess it by name.

### Windows: form factor, then the adapter's enumerator

Both are read from the endpoint's property store, `IMMDevice::OpenPropertyStore(STGM_READ)`, which
`wasapi/mod.rs` already opens for the friendly name. That store carries the adapter's
`DEVPKEY_Device_EnumeratorName`: `HDAUDIO` for an onboard codec and `USB` for a USB headset, in a
dump by one of Windows' audio developers
([Van Eerde, 2011](https://matthewvaneerde.wordpress.com/2011/06/13/how-to-enumerate-audio-endpoint-immdevice-properties-on-your-system/)).
Windows' own default-device heuristic ranks endpoints on the same enumerator, with the values
`BTHENUM`, `BTHHFENUM`, `USB`, `HDAUDIO`, `PCI` and `Unknown`
([Default Audio Endpoint Selection](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/default-audio-endpoint-selection)).

| Property                                                                                                                             | Value                                                                                | `transport` |
| ------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------ | ----------- |
| [`PKEY_AudioEndpoint_FormFactor`](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/ne-mmdeviceapi-endpointformfactor) | `DigitalAudioDisplayDevice`: "connects to an audio adapter over HDMI or DisplayPort" | `display`   |
| the same                                                                                                                             | `RemoteNetworkDevice`: one "the user accesses remotely through a network"            | `network`   |
| `DEVPKEY_Device_EnumeratorName`                                                                                                      | `ROOT`                                                                               | `virtual`   |
| the same                                                                                                                             | starts with `BTH` (`BTHENUM`, `BTHHFENUM`, and LE's)                                 | `bluetooth` |
| the same                                                                                                                             | `USB`                                                                                | `usb`       |
| the same                                                                                                                             | `HDAUDIO`, `INTELAUDIO`                                                              | `builtIn`   |
| the same                                                                                                                             | `PCI`                                                                                | `other`     |
| the same                                                                                                                             | anything else, or a read that fails                                                  | `unknown`   |

- **`ROOT` is virtual.** Microsoft defines a root-enumerated device as one "whose enumerator is
  ROOT" ([DevCon SetHwID](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/devcon-sethwid)):
  no bus found it, an installer made it. Virtual audio drivers install that way, at
  `ROOT\MEDIA\0000` and up, as Microsoft's own MSVAD sample does
  ([wdmaudiodev](https://ipv6.freelists.org/post/wdmaudiodev/EXTERNAL-Same-problem-with-WIN10-update-2004,11)),
  and Wine names its fake devices the same way to match the ones Voicemeeter creates
  ([wine-devel](https://list.winehq.org/archives/list/wine-devel@list.winehq.org/message/RRZBRJRXZM6AH3ZNADCER5HDQ6SUQ2D4)).
- **`INTELAUDIO` is built in.** It is the HD-Audio codec behind Intel's Smart Sound DSP on recent
  laptops, as in `INTELAUDIO\FUNC_01&VEN_10EC...` for a Realtek codec
  ([Supermicro FAQ 34929](https://www.supermicro.com/support/faqs/faq.php?faq=34929)).
- **`SWD` and every enumerator not listed are `unknown`.** A software-device driver could be either,
  and a guess of hardware is the failure this field exists to prevent.
- Form factor comes first because a display hangs off `HDAUDIO` on most machines, through the GPU's
  or the chipset's codec.

### Linux: the ALSA card's driver and the pcm's name

The Linux backend lists only `hw:` devices through ALSA (`alsa/mod.rs`), with no sound server, so
PipeWire's and PulseAudio's `device.bus` and `device.form_factor` are never in reach. The card's
driver string is: `snd_ctl_card_info_get_driver`, or the module behind
`/sys/class/sound/cardN/device/driver`. Neither opens a pcm.

| Card driver, or pcm                                 | Set in the kernel by                                                                                                                                                                                                                                                                                                                                                                          | `transport` |
| --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------- |
| `Loopback`, `Dummy`, `PCM-TEST Driver`              | [`aloop.c:1991`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/sound/drivers/aloop.c#L1991), [`dummy.c:1113`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/sound/drivers/dummy.c#L1113), [`pcmtest.c:590`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/sound/drivers/pcmtest.c#L590) | `virtual`   |
| `USB-Audio`                                         | [`usb/card.c:773`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/sound/usb/card.c#L773)                                                                                                                                                                                                                                                                     | `usb`       |
| a pcm named `HDMI n`, or the card driver `vc4-hdmi` | [`hdmi.c:1859`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/sound/hda/codecs/hdmi/hdmi.c#L1859), [`vc4_hdmi.c:2408`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/drivers/gpu/drm/vc4/vc4_hdmi.c#L2408)                                                                                                                | `display`   |
| `HDA-Intel`, any other pcm                          | [`controllers/intel.c:2083`](https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/sound/hda/controllers/intel.c#L2083)                                                                                                                                                                                                                                             | `builtIn`   |
| any other driver                                    |                                                                                                                                                                                                                                                                                                                                                                                               | `other`     |
| a card whose driver cannot be read                  |                                                                                                                                                                                                                                                                                                                                                                                               | `unknown`   |

`HDA-Intel` is every HD-Audio controller, AMD's and NVIDIA's included, so a GPU's card is
`HDA-Intel` with only `HDMI n` pcms, and the pcm rule reaches it first. Bluetooth never reaches
ALSA's `hw:` list (BlueZ hands it to a sound server or to BlueALSA's plugin pcm), so Linux lists no
`bluetooth`, `network` or `aggregate` device. Any other driver is `other` rather than `unknown`: a
kernel driver that makes a pcm card is hardware, the three above are the ones in `sound/drivers/`
that are not, and a Raspberry Pi's headphone jack lands here.

## What no OS can tell

Whether a display has speakers. CoreAudio says `HDMI` or `DisplayPort`, Windows says
`DigitalAudioDisplayDevice`, and ALSA names the pcm `HDMI n`, for a monitor with speakers and one
with only a headphone jack alike: each OS lists a display's audio when the display says it takes
audio, and none of the properties above says what is plugged in behind it. So `display` ranks after
the wired hardware, and the field does not try to say more.

## Bluetooth's number

RTINGS measured 173 ms on SBC and 166 ms on aptX for one pair of headphones
([RTINGS](https://www.rtings.com/headphones/learn/latency)). SoundGuys averaged 308 ms on SBC
across four Android phones ([SoundGuys](https://www.soundguys.com/android-bluetooth-latency-22732/)).
Both are several times the 20 to 30 ms budget, which is why it ranks after a display.

## Versioning

The new field is required on a type the package exports, so a patch cannot carry it: decision 0024
(#171) says a patch changes no exported type. It ships in the next minor. A host that only reads
the list needs no change. A host whose tests build a fake list by hand must add `transport`, the
same kind of change as `OutputInfo.refused` in 0.7.0, which 0024 lists as breaking, so the release
that ships it adds a row to that table.

## What a person owes

- On a Mac with Zoom and Teams installed, `outputDevices()` lists ZoomAudioDevice and Microsoft
  Teams Audio as `virtual`. #165 names the Zoom check.
- On Windows with VB-CABLE or Voicemeeter, the cable is `virtual`, and a laptop's speakers are
  `builtIn` through `HDAUDIO` or `INTELAUDIO`.
- On a monitor with no speakers, the output is listed as `display`, and no other property sets it
  apart from one with speakers. If one does, `display` can be split then.
