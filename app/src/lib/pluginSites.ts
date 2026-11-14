// Where to get a plugin: its maker's site when the vendor in the plugin's
// name is one of the well-known ones below, a web search for it otherwise.
// The list lives here and nothing in it comes from a session, so a host can't
// point a guest at an address of its choosing.

import type { FxRef } from "@/lib/plugins";

/** Vendor as REAPER shows it (lower case, company suffix dropped) → site. */
const SITES: Record<string, string> = {
  "2caudio": "2caudio.com",
  "acustica": "acustica-audio.com",
  "acustica audio": "acustica-audio.com",
  "airwindows": "airwindows.com",
  "ample sound": "amplesound.net",
  "antares": "antarestech.com",
  "antares audio technologies": "antarestech.com",
  "applied acoustics systems": "applied-acoustics.com",
  "arturia": "arturia.com",
  "audio damage": "audiodamage.com",
  "audio ease": "audioease.com",
  "audio modeling": "audiomodeling.com",
  "audiothing": "audiothing.net",
  "baby audio": "babyaud.io",
  "blue cat audio": "bluecataudio.com",
  "cableguys": "cableguys.com",
  "celemony": "celemony.com",
  "cherry audio": "cherryaudio.com",
  "d16 group": "d16.pl",
  "devious machines": "deviousmachines.com",
  "dmg audio": "dmgaudio.com",
  "eastwest": "soundsonline.com",
  "eventide": "eventideaudio.com",
  "fabfilter": "fabfilter.com",
  "gforce": "gforcesoftware.com",
  "gforce software": "gforcesoftware.com",
  "goodhertz": "goodhertz.com",
  "heavyocity": "heavyocity.com",
  "ik multimedia": "ikmultimedia.com",
  "image-line": "image-line.com",
  "izotope": "izotope.com",
  "kilohearts": "kilohearts.com",
  "klanghelm": "klanghelm.com",
  "korg": "korg.com",
  "lennardigital": "lennardigital.com",
  "line 6": "line6.com",
  "liquidsonics": "liquidsonics.com",
  "mastering the mix": "masteringthemix.com",
  "mcdsp": "mcdsp.com",
  "meldaproduction": "meldaproduction.com",
  "minimal audio": "minimal.audio",
  "modartt": "modartt.com",
  "native instruments": "native-instruments.com",
  "neural dsp": "neuraldsp.com",
  "nugen audio": "nugenaudio.com",
  "oeksound": "oeksound.com",
  "orchestral tools": "orchestraltools.com",
  "output": "output.com",
  "overloud": "overloud.com",
  "plugin alliance": "plugin-alliance.com",
  "polyverse": "polyversemusic.com",
  "polyverse music": "polyversemusic.com",
  "positive grid": "positivegrid.com",
  "pspaudioware": "pspaudioware.com",
  "pulsar audio": "pulsar.audio",
  "refx": "refx.com",
  "relab": "relabdevelopment.com",
  "relab development": "relabdevelopment.com",
  "reveal sound": "reveal-sound.com",
  "rob papen": "robpapen.com",
  "roland": "roland.com",
  "slate digital": "slatedigital.com",
  "softube": "softube.com",
  "solid state logic": "solidstatelogic.com",
  "sonarworks": "sonarworks.com",
  "sonible": "sonible.com",
  "sonic academy": "sonicacademy.com",
  "sonnox": "sonnox.com",
  "soundtoys": "soundtoys.com",
  "spectrasonics": "spectrasonics.net",
  "spitfire audio": "spitfireaudio.com",
  "ssl": "solidstatelogic.com",
  "steinberg": "steinberg.net",
  "steinberg media technologies": "steinberg.net",
  "sugar bytes": "sugar-bytes.de",
  "surge synth team": "surge-synthesizer.github.io",
  "synapse audio": "synapse-audio.com",
  "tal-togu audio line": "tal-software.com",
  "tbproaudio": "tbproaudio.de",
  "togu audio line": "tal-software.com",
  "tokyo dawn labs": "tokyodawn.net",
  "tone2": "tone2.com",
  "toontrack": "toontrack.com",
  "u-he": "u-he.com",
  "ujam": "ujam.com",
  "unfiltered audio": "unfilteredaudio.com",
  "universal audio": "uaudio.com",
  "valhalla dsp": "valhalladsp.com",
  "valhalladsp": "valhalladsp.com",
  "vienna symphonic library": "vsl.co.at",
  "vital audio": "vital.audio",
  "voxengo": "voxengo.com",
  "vsl": "vsl.co.at",
  "waves": "waves.com",
  "wavesfactory": "wavesfactory.com",
  "xfer records": "xferrecords.com",
  "xln audio": "xlnaudio.com",
  "youlean": "youlean.co",
  "zynaptiq": "zynaptiq.com",
};

// "Valhalla DSP, LLC", "Native Instruments GmbH", "Universal Audio (UADx)".
const COMPANY = /[\s,]+(?:gmbh|inc|llc|ltd|limited|corp|corporation|co|ab|bv|kg|ug|srl|sarl|s\.r\.l)\.?$/;

function vendorKey(vendor: string): string {
  let key = vendor.toLowerCase().replace(/\s+/g, " ").trim();
  for (let i = 0; i < 2; i++) key = key.replace(COMPANY, "").trim();
  return key.replace(/[.,]+$/, "").trim();
}

export interface PluginSite {
  /** The site's host name, "" for a search. */
  host: string;
  url: string;
}

export function pluginSite(p: FxRef): PluginSite {
  const host = p.vendor ? SITES[vendorKey(p.vendor)] : undefined;
  if (host) return { host, url: `https://${host}` };
  // A JS effect is named by its path: the file name is what people know it by.
  const js = p.fmt.toUpperCase() === "JS";
  const name = js ? (p.name.split("/").pop() ?? p.name) : p.name;
  const terms = [name, p.vendor, js ? "JSFX REAPER" : "plugin"].filter(Boolean).join(" ");
  return { host: "", url: `https://duckduckgo.com/?q=${encodeURIComponent(terms)}` };
}
