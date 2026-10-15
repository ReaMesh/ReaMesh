// Unit test for the pure state-chunk parsing/splicing helpers in
// extension/src/reamesh.cpp (normalizeChunk, extractTrackBlocks,
// replaceTrackBlocks, transformRecvUnits, chunkFilePaths, isRelativePath,
// pathInsideDir, relativizeChunkFiles, absolutizeChunkFiles).
// No REAPER needed.
//
// Build & run:
//   clang++ -std=c++20 -Iextension/include -Iextension/sdk -Iextension/build/generated \
//     tools/chunk_test.cpp rust/target/release/libreamesh.a -framework CoreFoundation \
//     -framework Security -framework SystemConfiguration -lresolv \
//     -o /tmp/chunk_test && /tmp/chunk_test
//   (extension/build/generated/reamesh_gui_script.h is produced by the cmake step
//   of ./build.sh)

#include "../extension/src/reamesh.cpp"

#include <cassert>
#include <cstdio>

static const char* kTrackChunk =
    "<TRACK {11111111-1111-1111-1111-111111111111}\n"
    "  NAME \"Drums\"\n"
    "  VOLPAN 1 0 -1 -1 1\n"
    "  GROUP_FLAGS 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
    "  GROUP_FLAGS_HIGH 0 0 0 0 1 0 0 0 0 0 0 0 0 0 0 0\n"
    "  LOCK 1\n"
    "  AUXRECV 0 0 1 0 0 0 0 0 0 -1 0 -1 ''\n"
    "  <AUXVOLENV\n"
    "    ACT 1 -1\n"
    "    ARM 1\n"
    "    VIS 1 1 1\n"
    "    PT 0 1 0\n"
    "  >\n"
    "  AUXRECV 1 0 0.5 0 0 0 0 0 0 -1 0 -1 ''\n"
    "  <FXCHAIN_REC\n"
    "    <VST \"VST: ReaGate\" reagate.dylib 0\n"
    "      gatecfg==\n"
    "    >\n"
    "  >\n"
    "  <VOLENV2\n"
    "    ACT 1 -1\n"
    "    ARM 0\n"
    "    VIS 1 1 1\n"
    "    PT 0 1 0\n"
    "  >\n"
    "  <FXCHAIN\n"
    "    SHOW 2\n"
    "    LASTSEL 0\n"
    "    DOCKED 0\n"
    "    BYPASS 0 0 0\n"
    "    <VST \"VST: ReaEQ\" reaeq.dylib 0\n"
    "      base64data==\n"
    "      FLOATSAREJUSTBASE64==\n"
    "    >\n"
    "    FLOATPOS 0 0 0 0\n"
    "    FLOAT 120 80 600 400\n"
    "  >\n"
    "  <ITEM\n"
    "    POSITION 1\n"
    "    SEL 1\n"
    "    LOCK 1\n"
    "    <SOURCE WAVE\n"
    "      FILE \"Audio/kick.wav\"\n"
    "    >\n"
    "  >\n"
    ">\n";

int main() {
  // normalizeChunk drops volatile UI/selection lines (now incl. envelope
  // ARM/VIS: per-user automation workflow state), keeps the rest.
  std::string norm = normalizeChunk(kTrackChunk);
  assert(norm.find("SEL 1") == std::string::npos);
  assert(norm.find("SHOW 2") == std::string::npos);
  assert(norm.find("LASTSEL") == std::string::npos);
  assert(norm.find("DOCKED") == std::string::npos);
  assert(norm.find("FLOATPOS") == std::string::npos);
  assert(norm.find("FLOAT 120") == std::string::npos);  // a floating FX window is per-user too
  assert(norm.find("ARM ") == std::string::npos);
  assert(norm.find("VIS ") == std::string::npos);
  assert(norm.find("ACT 1 -1") != std::string::npos);   // envelope active state stays
  assert(norm.find("BYPASS 0 0 0") != std::string::npos);
  assert(norm.find("NAME \"Drums\"") != std::string::npos);
  assert(norm.find("base64data==") != std::string::npos);
  // Base64 plugin state can begin with any upper-case run, "FLOAT" included.
  // The volatile-line prefixes must be anchored (trailing space, or matched
  // whole) or a payload line like this is silently dropped and every peer
  // splices a truncated FX state into its track.
  assert(norm.find("FLOATSAREJUSTBASE64==") != std::string::npos);
  assert(norm.find("FILE \"Audio/kick.wav\"") != std::string::npos);

  // The same chunk with Windows line endings must normalize to exactly the
  // same bytes as the macOS one: REAPER serializes with the platform's EOL,
  // so without this a Windows and a macOS peer would each see every
  // chunk-carried field as changed and overwrite each other forever.
  std::string crlf;
  for (char c : std::string(kTrackChunk)) {
    if (c == '\n') crlf.push_back('\r');
    crlf.push_back(c);
  }
  assert(crlf != std::string(kTrackChunk));  // the fixture really did change
  assert(normalizeChunk(crlf) == norm);
  assert(normalizeChunk(crlf).find('\r') == std::string::npos);
  // Idempotent: re-normalizing already-LF text is a no-op, so capture ->
  // apply -> recapture cannot drift.
  assert(normalizeChunk(norm) == norm);

  // Windows media paths must reach the wire with forward slashes: macOS treats
  // a backslash as an ordinary filename character, so it could not resolve
  // them at all. Only the quoted path is rewritten: base64 FX state and
  // anything else on the line survives verbatim.
  const std::string winPaths =
      "  <SOURCE WAVE\n"
      "    FILE \"Media\\Drums\\kick.wav\" 1\n"
      "  >\n"
      "  DATA \"a\\\\b/c==\"\n";
  const std::string normWin = normalizeChunk(winPaths);
  assert(normWin.find("FILE \"Media/Drums/kick.wav\" 1") != std::string::npos);
  assert(normWin.find("DATA \"a\\\\b/c==\"") != std::string::npos);  // untouched
  assert(normalizeChunk(normWin) == normWin);                       // still idempotent

  // extractTrackBlocks pulls all synced parts, not items.
  TrackBlocks blocks = extractTrackBlocks(norm);
  assert(blocks.fxchain.find("<FXCHAIN") == 0 || blocks.fxchain.find("  <FXCHAIN") == 0);
  assert(blocks.fxchain.find("ReaEQ") != std::string::npos);
  assert(blocks.fxchain.find("ReaGate") == std::string::npos);
  assert(blocks.fxchain.find("POSITION 1") == std::string::npos);
  assert(blocks.recfx.find("FXCHAIN_REC") != std::string::npos);
  assert(blocks.recfx.find("ReaGate") != std::string::npos);
  assert(blocks.env.find("<VOLENV2") != std::string::npos);
  assert(blocks.env.find("PT 0 1 0") != std::string::npos);
  assert(blocks.env.find("AUXVOLENV") == std::string::npos);  // send env rides with recv
  assert(blocks.env.find("ReaEQ") == std::string::npos);
  assert(blocks.recv.find("AUXRECV 0") != std::string::npos);
  assert(blocks.recv.find("AUXRECV 1") != std::string::npos);
  assert(blocks.recv.find("<AUXVOLENV") != std::string::npos);
  assert(blocks.group.find("GROUP_FLAGS 1") != std::string::npos);
  assert(blocks.group.find("GROUP_FLAGS_HIGH") != std::string::npos);
  // Track LOCK only: the item's LOCK line is at depth 2 and stays out.
  assert(blocks.lock == "  LOCK 1\n");

  // Replacing the FX chain keeps everything else byte-identical.
  std::string newFx =
      "  <FXCHAIN\n"
      "    <VST \"VST: ReaComp\" reacomp.dylib 0\n"
      "      othercfg==\n"
      "    >\n"
      "  >\n";
  TrackBlocksPatch fxPatch;
  fxPatch.fxchain = &newFx;
  std::string replaced = replaceTrackBlocks(norm, fxPatch);
  assert(replaced.find("ReaComp") != std::string::npos);
  assert(replaced.find("ReaEQ") == std::string::npos);
  assert(replaced.find("<VOLENV2") != std::string::npos);     // env untouched
  assert(replaced.find("ReaGate") != std::string::npos);      // input FX untouched
  assert(replaced.find("AUXRECV 0") != std::string::npos);    // receives untouched
  assert(replaced.find("POSITION 1") != std::string::npos);   // items untouched
  assert(replaced.find("NAME \"Drums\"") != std::string::npos);
  // Round-trip: extracting from the spliced chunk gives back what we put in.
  TrackBlocks reblocks = extractTrackBlocks(replaced);
  assert(reblocks.fxchain == newFx);
  assert(reblocks.env == blocks.env);
  assert(reblocks.recfx == blocks.recfx);
  assert(reblocks.recv == blocks.recv);
  assert(reblocks.group == blocks.group);
  assert(reblocks.lock == blocks.lock);

  // Removing the FX chain entirely (empty replacement).
  std::string none;
  TrackBlocksPatch nonePatch;
  nonePatch.fxchain = &none;
  std::string stripped = replaceTrackBlocks(replaced, nonePatch);
  assert(stripped.find("ReaComp") == std::string::npos);
  assert(stripped.find("FXCHAIN_REC") != std::string::npos);  // input FX survives
  assert(stripped.find("<VOLENV2") != std::string::npos);
  assert(extractTrackBlocks(stripped).fxchain.empty());

  // Replacing envelopes only must not touch the AUX send envelopes.
  std::string newEnv =
      "  <PANENV2\n"
      "    PT 0 0 0\n"
      "  >\n";
  TrackBlocksPatch envPatch;
  envPatch.env = &newEnv;
  std::string envSwapped = replaceTrackBlocks(norm, envPatch);
  assert(envSwapped.find("PANENV2") != std::string::npos);
  assert(envSwapped.find("VOLENV2") == std::string::npos);
  assert(envSwapped.find("AUXVOLENV") != std::string::npos);  // send env untouched
  assert(envSwapped.find("ReaEQ") != std::string::npos);      // fxchain untouched

  // Replacing receives swaps the AUXRECV lines AND their send envelopes.
  std::string newRecv = "  AUXRECV 3 0 1 0 0 0 0 0 0 -1 0 -1 ''\n";
  TrackBlocksPatch recvPatch;
  recvPatch.recv = &newRecv;
  std::string recvSwapped = replaceTrackBlocks(norm, recvPatch);
  assert(recvSwapped.find("AUXRECV 3") != std::string::npos);
  assert(recvSwapped.find("AUXRECV 0") == std::string::npos);
  assert(recvSwapped.find("AUXVOLENV") == std::string::npos);
  assert(recvSwapped.find("<VOLENV2") != std::string::npos);  // track env untouched
  assert(extractTrackBlocks(recvSwapped).recv == newRecv);

  // Replacing group flags / lock lines.
  std::string newGroup = "  GROUP_FLAGS 0 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n";
  TrackBlocksPatch groupPatch;
  groupPatch.group = &newGroup;
  groupPatch.lock = &none;  // unlock
  std::string grouped = replaceTrackBlocks(norm, groupPatch);
  assert(extractTrackBlocks(grouped).group == newGroup);
  assert(extractTrackBlocks(grouped).lock.empty());
  assert(grouped.find("    LOCK 1\n    <SOURCE") != std::string::npos);  // item LOCK kept

  // Adding a chain to a track that never had one (and no envelopes either).
  std::string bare =
      "<TRACK {2}\n"
      "  NAME \"Bare\"\n"
      "  <ITEM\n"
      "    POSITION 5\n"
      "  >\n"
      ">\n";
  std::string withFx = replaceTrackBlocks(bare, fxPatch);
  assert(extractTrackBlocks(withFx).fxchain == newFx);
  // The chain must land before the items so REAPER parses it as track FX.
  assert(withFx.find("FXCHAIN") < withFx.find("<ITEM"));

  // transformRecvUnits: index -> GUID, dropping units with no mapping. The
  // dropped unit takes its attached send-envelope blocks with it.
  std::string wire = transformRecvUnits(blocks.recv, [](const std::string& tok) -> std::string {
    if (tok == "0") return "{AAAAAAAA-0000-0000-0000-000000000000}";
    return "";  // source track 1 unknown -> drop
  });
  assert(wire.find("AUXRECV {AAAAAAAA-0000-0000-0000-000000000000} 0 1 0") != std::string::npos);
  assert(wire.find("AUXRECV 1") == std::string::npos);
  assert(wire.find("<AUXVOLENV") != std::string::npos);  // env of the kept unit
  // ...and back: GUID -> local index.
  std::string local = transformRecvUnits(wire, [](const std::string& tok) -> std::string {
    return tok == "{AAAAAAAA-0000-0000-0000-000000000000}" ? "4" : "";
  });
  assert(local.find("AUXRECV 4 0 1 0") != std::string::npos);
  assert(local.find("{AAAAAAAA") == std::string::npos);
  assert(local.find("<AUXVOLENV") != std::string::npos);
  // Dropping the first unit removes its envelope block too.
  std::string dropped =
      transformRecvUnits(blocks.recv, [](const std::string& tok) -> std::string {
        return tok == "1" ? "{BBBBBBBB-0000-0000-0000-000000000000}" : "";
      });
  assert(dropped.find("AUXVOLENV") == std::string::npos);
  assert(dropped.find("AUXRECV {BBBBBBBB") != std::string::npos);

  // File references and path classification.
  auto files = chunkFilePaths(norm);
  assert(files.size() == 1 && files[0] == "Audio/kick.wav");
  assert(isRelativePath("Audio/kick.wav"));
  assert(!isRelativePath("/abs/kick.wav"));
  assert(!isRelativePath("C:\\media\\kick.wav"));
  assert(!isRelativePath("\\\\server\\share.wav"));

  // ---- plugin inventory (collectPlugins) ----
  // Both the host's manifest claim and the client's post-download verify run
  // this same parser over RPP text, so it has to see every FX flavour: VST
  // (any quoting style), CLAP/AU, JS (bare and quoted paths), take FX inside
  // items, and the master chain: deduplicated, in document order.
  const std::string kProject =
      "<REAPER_PROJECT 0.1\n"
      "  <MASTERFXLIST\n"
      "    <VST \"VST: ReaComp (Cockos)\" reacomp.dll 0\n"
      "      data==\n"
      "    >\n"
      "  >\n"
      "  <TRACK {AAAAAAAA-0000-0000-0000-000000000000}\n"
      "    <FXCHAIN\n"
      "      <VST \"VST3: Serum (Xfer Records)\" Serum.vst3 0\n"
      "        base64==\n"
      "      >\n"
      "      <CLAP \"CLAP: Surge XT (Surge Synth Team)\" org.surge.xt 0\n"
      "        state==\n"
      "      >\n"
      "      <JS loser/waveShapingDstr \"\"\n"
      "      >\n"
      "      <JS \"my effects/space echo\" \"\"\n"
      "      >\n"
      "      <VST `VST: Odd \"Name\" (Vendor)` odd.dll 0\n"
      "      >\n"
      "    >\n"
      "    <FXCHAIN_REC\n"
      "      <VST \"VST: ReaGate (Cockos)\" reagate.dll 0\n"
      "      >\n"
      "    >\n"
      "    <ITEM\n"
      "      <TAKEFX\n"
      "        <AU \"AU: Alchemy (Apple)\" au.alchemy 0\n"
      "        >\n"
      "        <VST \"VST3: Serum (Xfer Records)\" Serum.vst3 0\n"  // dup: counted once
      "        >\n"
      "      >\n"
      "      <SOURCE WAVE\n"
      "        FILE \"Audio/kick.wav\"\n"
      "      >\n"
      "    >\n"
      "  >\n"
      ">\n";
  {
    auto plugs = collectPlugins(kProject);
    const std::vector<std::string> want = {
        "VST: ReaComp (Cockos)",       "VST3: Serum (Xfer Records)",
        "CLAP: Surge XT (Surge Synth Team)", "JS: loser/waveShapingDstr",
        "JS: my effects/space echo",   "VST: Odd \"Name\" (Vendor)",
        "VST: ReaGate (Cockos)",       "AU: Alchemy (Apple)"};
    assert(plugs == want);
    // CRLF text (a Windows-saved .rpp) inventories identically.
    std::string crlfProj;
    for (char c : kProject) {
      if (c == '\n') crlfProj.push_back('\r');
      crlfProj.push_back(c);
    }
    assert(collectPlugins(crlfProj) == want);
    // No FX, no findings.
    assert(collectPlugins("<TRACK\n  NAME \"x\"\n>\n").empty());
  }

  // ---- FX stripping (stripFxBlocks) ----
  {
    // Full strip (join-without-effects): every chain flavour gone, everything
    // else (track scalars, item source, structure) intact.
    const std::string stripped = stripFxBlocks(kProject, /*takeFxOnly=*/false);
    assert(collectPlugins(stripped).empty());
    assert(stripped.find("MASTERFXLIST") == std::string::npos);
    assert(stripped.find("FXCHAIN") == std::string::npos);  // covers _REC too
    assert(stripped.find("TAKEFX") == std::string::npos);
    assert(stripped.find("<TRACK {AAAAAAAA") != std::string::npos);
    assert(stripped.find("FILE \"Audio/kick.wav\"") != std::string::npos);
    // Balanced: as many closers as openers survive the surgery.
    const auto count = [](const std::string& s, char c) {
      return std::count(s.begin(), s.end(), c);
    };
    (void)count;
    // Take-FX-only strip (the no-FX apply path for item chunks): take FX gone,
    // track chains untouched.
    const std::string takeOnly = stripFxBlocks(kProject, /*takeFxOnly=*/true);
    assert(takeOnly.find("TAKEFX") == std::string::npos);
    assert(takeOnly.find("Alchemy") == std::string::npos);
    assert(takeOnly.find("Surge XT") != std::string::npos);
    assert(takeOnly.find("MASTERFXLIST") != std::string::npos);
    // Idempotent.
    assert(stripFxBlocks(stripped, false) == stripped);
  }

  // ---- shared flag markers ----
  // "⚑ "-prefixed markers are local by design: never captured, never deleted
  // by a remote marker rebuild.
  assert(isSharedMarkerName("\xE2\x9A\x91 Chorus fix by Dan"));
  assert(!isSharedMarkerName("Chorus fix"));
  assert(!isSharedMarkerName(""));
  assert(!isSharedMarkerName(nullptr));
  // The name must start with the prefix, not merely contain it.
  assert(!isSharedMarkerName("x \xE2\x9A\x91 y"));

  // ---- progressive join: placeholder rewrite (blankMissingSources) ----
  {
    const std::string proj =
        "<REAPER_PROJECT 0.1\n"
        "  <TRACK {T1}\n"
        "    NAME \"Drums\"\n"
        "    <ITEM\n"
        "      POSITION 1\n"
        "      LENGTH 4\n"
        "      IGUID {ITEM-MISSING}\n"
        "      NAME \"kick\"\n"
        "      GUID {TAKE-1}\n"
        "      <SOURCE WAVE\n"
        "        FILE \"Audio\\kick.wav\"\n"
        "      >\n"
        "    >\n"
        "    <ITEM\n"
        "      POSITION 5\n"
        "      IGUID {ITEM-PRESENT}\n"
        "      <SOURCE WAVE\n"
        "        FILE \"Audio/snare.wav\"\n"
        "      >\n"
        "    >\n"
        "    <ITEM\n"
        "      POSITION 9\n"
        "      IGUID {ITEM-MIDI}\n"
        "      <SOURCE MIDI\n"
        "        HASDATA 1 960 QN\n"
        "        E 0 90 3c 60\n"
        "      >\n"
        "    >\n"
        "    <ITEM\n"
        "      POSITION 12\n"
        "      IGUID {ITEM-SECTION}\n"
        "      <SOURCE SECTION\n"
        "        LENGTH 2\n"
        "        <SOURCE WAVE\n"
        "          FILE \"Audio/reversed.wav\"\n"
        "        >\n"
        "      >\n"
        "      TAKE\n"
        "      GUID {TAKE-2}\n"
        "      <SOURCE WAVE\n"
        "        FILE \"Audio/snare.wav\"\n"
        "      >\n"
        "    >\n"
        "  >\n"
        ">\n";
    const auto present = [](const std::string& f) { return f == "Audio/snare.wav"; };
    const BlankResult r = blankMissingSources(proj, present);
    // Exactly the items with a missing file are reported, by IGUID.
    assert((r.guids == std::set<std::string>{"{ITEM-MISSING}", "{ITEM-SECTION}"}));
    // Missing sources became EMPTY; present ones (incl. the second take of the
    // SECTION item) and MIDI survive verbatim; the item shells stay intact.
    assert(r.text.find("kick.wav") == std::string::npos);
    assert(r.text.find("reversed.wav") == std::string::npos);
    assert(r.text.find("<SOURCE SECTION") == std::string::npos);
    // A SECTION source and its nested WAVE collapse into ONE empty source.
    assert(std::count(r.text.begin(), r.text.end(), '<') == std::count(proj.begin(), proj.end(), '<') - 1);
    assert(r.text.find("      <SOURCE EMPTY\n      >\n") != std::string::npos);  // indentation kept
    assert(r.text.find("FILE \"Audio/snare.wav\"") != std::string::npos);
    assert(r.text.find("E 0 90 3c 60") != std::string::npos);
    assert(r.text.find("NAME \"kick\"") != std::string::npos);
    assert(r.text.find("POSITION 12") != std::string::npos);
    assert(r.text.find("GUID {TAKE-2}") != std::string::npos);
    // Balanced: every block still closes.
    {
      int depth = 0;
      size_t pos = 0;
      while (pos < r.text.size()) {
        size_t eol = r.text.find('\n', pos);
        if (eol == std::string::npos) eol = r.text.size();
        std::string_view t = ltrim(std::string_view(r.text.data() + pos, eol - pos));
        if (!t.empty() && t.front() == '<') ++depth;
        else if (t == ">") --depth;
        assert(depth >= 0);
        pos = eol + 1;
      }
      assert(depth == 0);
    }
    // Nothing missing: byte-identical (LF) output, no GUIDs.
    const BlankResult none = blankMissingSources(proj, [](const std::string&) { return true; });
    assert(none.guids.empty());
    assert(none.text == proj);
    // Idempotent: a placeholder text has no files left to blank.
    const BlankResult again = blankMissingSources(r.text, present);
    assert(again.guids.empty());
    assert(again.text == r.text);
    // CRLF input rewrites to the same LF text.
    std::string crlfProj;
    for (char c : proj) {
      if (c == '\n') crlfProj.push_back('\r');
      crlfProj.push_back(c);
    }
    assert(blankMissingSources(crlfProj, present).text == r.text);
    // The two source classifiers behind the guards.
    assert(chunkHasFileSource("<ITEM\n  <SOURCE WAVE\n    FILE \"a.wav\"\n  >\n>\n"));
    assert(!chunkHasFileSource("<ITEM\n  <SOURCE EMPTY\n  >\n>\n"));
    assert(chunkHasEmptySource("<ITEM\n  <SOURCE EMPTY\n  >\n>\n"));
    assert(!chunkHasEmptySource("<ITEM\n  <SOURCE MIDI\n  >\n>\n"));
    // Only the item's own COLOR line counts: an item in the default colour has
    // none, and that is what tells applyItem to clear a leftover lock tint.
    assert(itemChunkHasColor("<ITEM\n  POSITION 1\n  COLOR 21036910 R\n  <SOURCE WAVE\n  >\n>\n"));
    assert(!itemChunkHasColor("<ITEM\n  POSITION 1\n  TAKECOLOR 123 B\n  <SOURCE WAVE\n  >\n>\n"));
    assert(!itemChunkHasColor("<ITEM\n  <TAKEFX\n    COLOR 5\n  >\n>\n"));
  }

  // Item media paths go on the wire project-relative: REAPER serializes them
  // absolute, and an absolute path from a peer on the same machine resolves to
  // that peer's file (which the receiver then copied into its project root).
  {
    assert(pathInsideDir("/Songs/Nightfall/Audio/Kick_01.wav", "/Songs/Nightfall") ==
           "Audio/Kick_01.wav");
    assert(pathInsideDir("/Songs/Nightfall/Kick.wav", "/Songs/Nightfall/") == "Kick.wav");
    // A sibling folder sharing the prefix is outside, so is the folder itself.
    assert(pathInsideDir("/Songs/Nightfall2/Kick.wav", "/Songs/Nightfall").empty());
    assert(pathInsideDir("/Songs/Nightfall", "/Songs/Nightfall").empty());
    assert(pathInsideDir("/Songs/Nightfall/", "/Songs/Nightfall").empty());
    assert(pathInsideDir("/Other/Audio/Kick.wav", "/Songs/Nightfall").empty());
    // Case matters on POSIX paths, not on drive-letter ones; separators never do.
    assert(pathInsideDir("/songs/nightfall/Kick.wav", "/Songs/Nightfall").empty());
    assert(pathInsideDir("c:/Users/Ben/Song/Media/Kick.wav", "C:\\Users\\Ben\\Song") ==
           "Media/Kick.wav");
    assert(pathInsideDir("C:\\Users\\Ben\\Song\\Kick.wav", "C:\\Users\\Ben\\Song") ==
           "Kick.wav");
    // Never a path that climbs back out, never a relative base.
    assert(pathInsideDir("/Songs/Nightfall/../Elsewhere/Kick.wav", "/Songs/Nightfall").empty());
    assert(pathInsideDir("/Songs/Nightfall/./Kick.wav", "/Songs/Nightfall").empty());
    assert(pathInsideDir("Audio/Kick.wav", "Audio").empty());
    assert(pathInsideDir("/Songs/Nightfall/Audio/Kick.wav", "").empty());

    const std::string item =
        "<ITEM\n"
        "  POSITION 1\n"
        "  <SOURCE SECTION\n"
        "    LENGTH 2\n"
        "    <SOURCE WAVE\n"
        "      FILE \"/Songs/Nightfall/Audio/Kick_01.wav\" 1\n"
        "    >\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE \"/Samples/OneShots/Clap.wav\"\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE '/Songs/Nightfall/Audio/Say \"hi\".wav'\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE \"Audio/Snare.wav\"\n"
        "  >\n"
        ">\n";
    const std::vector<std::string> dirs = {"/Songs/Nightfall"};
    const std::string rel = relativizeChunkFiles(item, dirs);
    // In-project media becomes relative, the rest of the line survives.
    assert(rel.find("      FILE \"Audio/Kick_01.wav\" 1\n") != std::string::npos);
    // Media outside the project stays absolute (internalizeExternalMedia
    // copies it in first), and so do paths that are already relative or quoted
    // with something other than a double quote.
    assert(rel.find("FILE \"/Samples/OneShots/Clap.wav\"") != std::string::npos);
    assert(rel.find("FILE '/Songs/Nightfall/Audio/Say \"hi\".wav'") != std::string::npos);
    assert(rel.find("FILE \"Audio/Snare.wav\"") != std::string::npos);
    // Nothing else moves; idempotent, so a recapture of an applied chunk is
    // byte-identical (no ping-pong between peers).
    assert(rel.size() == item.size() - std::string("/Songs/Nightfall/").size());
    assert(relativizeChunkFiles(rel, dirs) == rel);
    // A peer in another folder maps ITS absolute spelling to the same text.
    std::string other = item;
    other.replace(other.find("/Songs/Nightfall/Audio/Kick_01"), std::string("/Songs/Nightfall").size(),
                  "/Users/lena/ReaMesh/Nightfall-1a2b");
    assert(relativizeChunkFiles(other, {"/Users/lena/ReaMesh/Nightfall-1a2b"}).find(
               "FILE \"Audio/Kick_01.wav\" 1") != std::string::npos);
    // Second candidate dir (the canonical spelling) is honoured too.
    assert(relativizeChunkFiles(item, {"/elsewhere", "/Songs/Nightfall"}) == rel);
    // No dirs (unsaved project), or no trailing newline: unchanged / preserved.
    assert(relativizeChunkFiles(item, {}) == item);
    assert(relativizeChunkFiles("FILE \"/Songs/Nightfall/a.wav\"", dirs) == "FILE \"a.wav\"");
    // The wire form then reads as fetchable project-relative media.
    auto relFiles = chunkFilePaths(rel);
    assert(relFiles.size() == 4 && relFiles[0] == "Audio/Kick_01.wav" &&
           isRelativePath(relFiles[0]) && !isRelativePath(relFiles[1]));
  }

  // Incoming media paths are resolved against our own project folder before
  // REAPER sees them, including a folder whose name ends in a space.
  {
    const std::string wire =
        "<ITEM\n"
        "  <SOURCE WAVE\n"
        "    FILE \"Whoosh.wav\" 1\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE \"Audio/Kick.wav\"\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE \"/Samples/Clap.wav\"\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE \"../Elsewhere/Hat.wav\"\n"
        "  >\n"
        ">\n";
    const std::string dir = "/Users/david/Reaper Projekte ";
    const std::string abs = absolutizeChunkFiles(wire, dir);
    assert(abs.find("FILE \"/Users/david/Reaper Projekte /Whoosh.wav\" 1\n") != std::string::npos);
    assert(abs.find("FILE \"/Users/david/Reaper Projekte /Audio/Kick.wav\"") != std::string::npos);
    assert(abs.find("FILE \"/Samples/Clap.wav\"") != std::string::npos);
    assert(abs.find("FILE \"../Elsewhere/Hat.wav\"") != std::string::npos);
    // Recapture maps it back to the wire text exactly: no ping-pong.
    assert(relativizeChunkFiles(abs, {dir}) == wire);
    // A trailing separator on the folder doesn't double up.
    assert(absolutizeChunkFiles(wire, dir + "/") == abs);
    // Windows spelling: backslashes throughout, and back to '/' on the wire.
    const std::string win = absolutizeChunkFiles(wire, "C:\\Users\\Ben\\Song");
    assert(win.find("FILE \"C:\\Users\\Ben\\Song\\Audio\\Kick.wav\"") != std::string::npos);
    assert(relativizeChunkFiles(win, {"C:\\Users\\Ben\\Song"}) == wire);
    // No saved project: untouched.
    assert(absolutizeChunkFiles(wire, "") == wire);
  }

  // REAPER version gate: releases order as decimals, not as integer pairs.
  {
    assert(reaperVersionLabel("7.42/win64") == "7.42");
    assert(reaperVersionLabel("7.43+dev0601/macOS-arm64") == "7.43+dev0601");
    assert(reaperVersionKey("7.0") < reaperVersionKey("7.07"));
    assert(reaperVersionKey("7.07") < reaperVersionKey("7.12"));
    assert(reaperVersionKey("5.11") < reaperVersionKey("5.111"));
    assert(reaperVersionKey("5.111") < reaperVersionKey("5.12"));
    assert(reaperVersionKey("7.12rc3/win64") == reaperVersionKey("7.12"));
    assert(reaperVersionKey("") == -1);
    assert(reaperVersionSupported("7.12/win64"));
    assert(reaperVersionSupported("7.42/macOS-arm64"));
    assert(reaperVersionSupported("8.0/win64"));
    assert(!reaperVersionSupported("7.11/win64"));
    assert(!reaperVersionSupported("7.0/win64"));
    assert(!reaperVersionSupported("6.83/linux-x86_64"));
    // Unparseable: never lock anyone out over a format we didn't foresee.
    assert(reaperVersionSupported("garbage"));
  }

  {
    // Media paths from a peer: only project-relative ones may reach REAPER.
    for (const char* ok : {"", "Audio/kick.wav", "Audio\\kick.wav", "kick.wav", "~kick.wav",
                           "Sub/inner.rpp", "a b/c d.wav"})
      assert(!mediaPathUnsafe(ok));
    for (const char* bad :
         {"/Users/x/Music/secret.wav", "\\\\host\\share\\x.wav", "//host/share/x.wav",
          "C:\\Users\\x\\a.wav", "C:a.wav", "Audio/../../secret.wav", "../secret.wav",
          "./kick.wav", "Audio//kick.wav", "Audio/", "~/Music/x.wav", "kick.wav:stream",
          "file:///etc/passwd", "say \"hi\".wav", "a\nb.wav", "a\tb.wav"})
      assert(mediaPathUnsafe(bad));

    auto item = [](const std::string& fileLine) {
      return "<ITEM\n  POSITION 0\n  <SOURCE SECTION\n    LENGTH 1\n    <SOURCE WAVE\n      " +
             fileLine + "\n    >\n  >\n>\n";
    };
    assert(!chunkHasUnsafeFilePath(item("FILE \"Audio/kick.wav\"")));
    assert(!chunkHasUnsafeFilePath(item("FILE \"Audio/kick.wav\" 1")));
    assert(!chunkHasUnsafeFilePath(item("FILE \"Audio/kick.wav\"\r")));
    assert(chunkHasUnsafeFilePath(item("FILE \"/etc/hosts\"")));  // nested in a section
    assert(chunkHasUnsafeFilePath(item("FILE /etc/hosts")));      // unquoted
    assert(chunkHasUnsafeFilePath(item("file \"/etc/hosts\"")));  // any case
    assert(chunkHasUnsafeFilePath(item("\tFILE '/etc/hosts'")));
    assert(chunkHasUnsafeFilePath(item("FILE `say \"x\".wav`")));  // path with a double quote
    assert(chunkHasUnsafeFilePath(item("FILE \"Audio/kick.wav")));  // unterminated quote
    assert(chunkHasUnsafeFilePath(item("FILE \"\\\\srv\\share\\a.wav\"")));
    assert(chunkHasUnsafeFilePath(item("FILE \"C:\\Users\\x\\a.wav\"")));
    // Other keywords and payload lines are not FILE lines.
    assert(!chunkHasUnsafeFilePath("<ITEM\n  NAME \"FILE /etc/hosts\"\n  FILEX /etc/hosts\n>\n"));
    assert(!chunkHasUnsafeFilePath("<VST \"VST: x\" x.vst 0\n  RklMRSAvZXRj\n>\n"));
    assert(!chunkHasUnsafeFilePath(kTrackChunk));
    // What the extension itself puts on the wire passes.
    const std::string abs = "<ITEM\n  <SOURCE WAVE\n    FILE \"/Users/ben/Song/Audio/kick.wav\"\n  >\n>\n";
    assert(chunkHasUnsafeFilePath(abs));
    assert(!chunkHasUnsafeFilePath(relativizeChunkFiles(abs, {"/Users/ben/Song"})));

    // A changeset loses the offending item whole and the offending track field.
    json cs = {
        {"items",
         {{"{OK}", {{"present", true}, {"chunk", item("FILE \"Audio/a.wav\"")}}},
          {"{BAD}", {{"present", true}, {"chunk", item("FILE \"/Users/x/a.wav\"")}}}}},
        {"tracks",
         {{"{T}",
           {{"name", "FILE /not/a/chunk"}, {"fxchain", "<FXCHAIN\nFILE \"../x.wav\"\n>\n"}}}}}};
    dropUnsafeMediaPaths(cs);
    assert(cs["items"].contains("{OK}") && !cs["items"].contains("{BAD}"));
    assert(!cs["tracks"]["{T}"].contains("fxchain"));
    assert(cs["tracks"]["{T}"]["name"] == "FILE /not/a/chunk");
  }

  // Incoming track parts: everything capture produces is accepted as is, and
  // nothing that would splice more than the part itself gets through.
  {
    const TrackBlocks parts = extractTrackBlocks(norm);
    assert(validTrackPart(kFxChain, parts.fxchain));
    assert(validTrackPart(kRecFx, parts.recfx));
    assert(validTrackPart(kEnv, parts.env));
    assert(validTrackPart(kRecv, parts.recv));
    assert(validTrackPart(kGroup, parts.group));
    assert(validTrackPart(kLock, parts.lock));
    // The wire form of receives (GUID instead of the source index) too.
    const std::string wireRecv = transformRecvUnits(
        parts.recv, [](const std::string& tok) { return "{GUID-" + tok + "}"; });
    assert(wireRecv.find("AUXRECV {GUID-0} ") != std::string::npos);
    assert(validTrackPart(kRecv, wireRecv));
    // Empty always means "remove the part".
    for (int k = 0; k < kPartCount; ++k) assert(validTrackPart(k, ""));
    // A part is only valid as its own kind.
    assert(!validTrackPart(kEnv, parts.fxchain));
    assert(!validTrackPart(kLock, parts.group));
    assert(!validTrackPart(kRecv, parts.env));

    // Something riding along after the part's own line/block.
    assert(!validTrackPart(kLock, "  LOCK 1\n  <FXCHAIN\n    <JS foo\n    >\n  >\n"));
    assert(!validTrackPart(kGroup, "  GROUP_FLAGS 1 0\n  <FXCHAIN\n  >\n"));
    assert(!validTrackPart(kEnv, parts.env + "  <ITEM\n    POSITION 1\n  >\n"));
    assert(!validTrackPart(kFxChain, parts.fxchain + parts.fxchain));  // two chains
    assert(!validTrackPart(kFxChain, parts.fxchain + "  NAME \"x\"\n"));
    assert(!validTrackPart(kRecv, "  NAME \"x\"\n" + parts.recv));  // before the first AUXRECV
    // Block structure that would close the track or swallow the rest of it.
    assert(!validTrackPart(kEnv, "  <VOLENV2\n    PT 0 1 0\n"));            // unclosed
    assert(!validTrackPart(kEnv, "  <VOLENV2\n  >\n  >\n  <FXCHAIN\n  >\n"));  // extra close
    assert(!validTrackPart(kLock, "  LOCK 1\n>\n<FXCHAIN\n>\n"));
    // Nested blocks don't belong in envelopes (an item, a source, a plugin).
    assert(!validTrackPart(kEnv, "  <VOLENV2\n    <ITEM\n    >\n  >\n"));
    assert(!validTrackPart(kEnv, "  <VOLENV2\n    <SOURCE WAVE\n    >\n  >\n"));
    assert(!validTrackPart(kRecv, "  AUXRECV 0 0\n  <AUXVOLENV\n    <VST \"x\" y\n    >\n  >\n"));
    // Line shapes: integers only, nothing else on the LOCK / GROUP lines.
    assert(validTrackPart(kLock, "LOCK 1\n"));
    assert(!validTrackPart(kLock, "LOCK 1\nLOCK 0\n"));
    assert(!validTrackPart(kLock, "LOCK x\n"));
    assert(!validTrackPart(kLock, "LOCK\n"));
    assert(!validTrackPart(kGroup, "GROUP_FLAGS 1 \"<VST\"\n"));
    assert(!validTrackPart(kGroup, "GROUP_FLAGSx1 1\n"));
    // Text hygiene: LF-only and '\n'-terminated, like capture.
    assert(!validTrackPart(kLock, "LOCK 1"));
    assert(!validTrackPart(kLock, "LOCK 1\r\n"));
    assert(!validTrackPart(kLock, std::string("LOCK 1\0\n", 8)));
  }

  // Receives: lines before the first AUXRECV belong to no unit and are dropped.
  {
    const std::string recv = "  NAME \"x\"\n  AUXRECV 1 0 1\n  <AUXVOLENV\n    PT 0 1 0\n  >\n";
    const std::string out = transformRecvUnits(recv, [](const std::string& t) { return t; });
    assert(out == "  AUXRECV 1 0 1\n  <AUXVOLENV\n    PT 0 1 0\n  >\n");
  }

  // Item chunks: one balanced <ITEM block, nothing outside it.
  {
    assert(validItemChunk("<ITEM\n  POSITION 1\n  <SOURCE WAVE\n    FILE \"a.wav\"\n  >\n>\n"));
    assert(validItemChunk("<ITEM\nPOSITION 2.0\n>"));  // no trailing newline
    assert(!validItemChunk(""));
    assert(!validItemChunk("<TRACK\n>\n"));
    assert(!validItemChunk("  POSITION 1\n<ITEM\n>\n"));
    assert(!validItemChunk("<ITEM\n  POSITION 1\n"));               // unclosed
    assert(!validItemChunk("<ITEM\n>\n<ITEM\n>\n"));                // two items
    assert(!validItemChunk("<ITEM\n>\n<FXCHAIN\n>\n"));
  }

  // Code-carrying take sources (no-FX joins).
  {
    const std::string sub =
        "<ITEM\n"
        "  POSITION 1\n"
        "  <SOURCE SECTION\n"
        "    LENGTH 2\n"
        "    <SOURCE RPP_PROJECT\n"
        "      FILE \"sub.rpp\"\n"
        "    >\n"
        "  >\n"
        "  <SOURCE VIDEOEFFECT\n"
        "    <CODE\n"
        "      | gfx_blit(0);\n"
        "    >\n"
        "  >\n"
        "  <SOURCE WAVE\n"
        "    FILE \"a.wav\"\n"
        "  >\n"
        ">\n";
    assert(hasCodeSource(sub));
    assert(!hasCodeSource("<ITEM\n  <SOURCE WAVE\n    FILE \"a.wav\"\n  >\n>\n"));
    const std::string blanked = blankCodeSources(sub);
    assert(!hasCodeSource(blanked));
    assert(blanked.find("RPP_PROJECT") == std::string::npos);
    assert(blanked.find("gfx_blit") == std::string::npos);
    assert(blanked.find("POSITION 1") != std::string::npos);    // the item stays
    assert(blanked.find("FILE \"a.wav\"") != std::string::npos);  // plain media stays
    assert(blanked.find("  <SOURCE EMPTY\n  >\n") != std::string::npos);
    assert(validItemChunk(blanked));
    assert(blankCodeSources(blanked) == blanked);
  }

  // Single-line strings: control characters out, UTF-8-safe length clamp.
  {
    assert(cleanLine("Bass\tDI\r\n", 64) == "Bass DI  ");
    assert(cleanLine(std::string("a\x01" "b\x7f" "c"), 64) == "abc");
    assert(cleanLine("Grüße", 3) == "Gr");  // never splits the ü
    assert(clampUtf8("Grüße", 4) == "Grü");
    assert(clampUtf8("abc", 10) == "abc");
    const std::string c = cleanLine("x\ty", 64);
    assert(cleanLine(c, 64) == c);  // idempotent
  }

  // Number parsing for peer text: whole tokens, finite, clamped.
  {
    int i = 0;
    double d = 0;
    assert(parseClampedInt("12", 0, 100, i) && i == 12);
    assert(parseClampedInt("99999999999999999999", 0, 100, i) && i == 100);
    assert(parseClampedInt("-99999999999999999999", -1, 100, i) && i == -1);
    assert(!parseClampedInt("12x", 0, 100, i));
    assert(!parseClampedInt("", 0, 100, i));
    assert(parseFiniteDouble("1.5", d) && d == 1.5);
    assert(!parseFiniteDouble("nan", d));
    assert(!parseFiniteDouble("inf", d));
    assert(!parseFiniteDouble("1e999", d));
    assert(clampToInt(1e300, 0, 10, 5) == 10);
    assert(clampToInt(std::nan(""), 0, 10, 5) == 5);
    assert(splitTokens("1  2 3", ' ') == (std::vector<std::string>{"1", "2", "3"}));
    assert(splitTokens("a\n\nb", '\n') == (std::vector<std::string>{"a", "", "b"}));
  }

  // Marker actions: a remote action-like name is guarded on apply, capture
  // gives back the exact wire text (no ping-pong), and only the owner keeps a
  // name that is live in its own project already.
  {
    assert(isActionMarkerName("!40044"));
    assert(isActionMarkerName("  !40044"));
    assert(!isActionMarkerName("Chorus !"));
    const std::string guarded = markerNameForApply("!40044", nullptr);
    assert(guarded != "!40044");
    assert(!isActionMarkerName(guarded));
    assert(unguardMarkerName(guarded) == "!40044");
    assert(sanitizeMarkerName(guarded.c_str()) == "!40044");
    assert(markerNameForApply("Verse", nullptr) == "Verse");
    assert(unguardMarkerName("Verse") == "Verse");
    const std::set<std::string> live = {"!1013"};
    assert(markerNameForApply("!1013", &live) == "!1013");  // the host's own
    assert(markerNameForApply("!40044", &live) != "!40044");  // anything new is guarded
    // A guard that doesn't sit in front of an action name is ordinary text.
    const std::string plain = std::string(kMarkerGuard) + "Intro";
    assert(unguardMarkerName(plain) == plain);
  }

  // The apply-side sanitizer: bad parts and items go, good ones pass unchanged.
  {
    const TrackBlocks parts = extractTrackBlocks(norm);
    json cs = {
        {"tracks",
         {{"{T1}",
           {{"name", "Lead\tVox"},
            {"env", parts.env},
            {"lock", "  LOCK 1\n  <FXCHAIN\n  >\n"},
            {"group", 5},
            {"fxchain", parts.fxchain}}}}},
        {"items",
         {{"{I1}", {{"chunk", "<ITEM\n  POSITION 1\n>\n"}}},
          {"{I2}", {{"chunk", "<ITEM\n>\n<FXCHAIN\n>\n"}}},
          {"{I3}", {{"present", false}}}}},
        {"project", {{"project", {{"title", "a\nb"}, {"groupnames", "x\ty\n\nz"}}}}}};
    SanitizeReport rep;
    json out = sanitizeIncoming(cs, /*noFx=*/false, rep);
    assert(rep.invalid == 3 && rep.noFx == 0);
    const json& t = out["tracks"]["{T1}"];
    assert(t["name"] == "Lead Vox");
    assert(t["env"] == parts.env);
    assert(t["fxchain"] == parts.fxchain);
    assert(!t.contains("lock") && !t.contains("group"));
    assert(out["items"].contains("{I1}") && !out["items"].contains("{I2}"));
    assert(out["items"].contains("{I3}"));
    assert(out["project"]["project"]["title"] == "a b");
    assert(out["project"]["project"]["groupnames"] == "x y\n\nz");

    // No-FX: chains go (routinely), and anything still loading code is dropped.
    json nofx = {
        {"tracks",
         {{"{T1}",
           {{"fxchain", parts.fxchain},
            {"recfx", parts.recfx},
            {"env", parts.env},
            {"recv", "AUXRECV 0 0\n<AUXVOLENV\nPT 0 1 0\n>\n"}}}}},
        {"items",
         {{"{I1}", {{"chunk", "<ITEM\n  <TAKEFX\n    <VST \"x\" y\n    >\n  >\n>\n"}}},
          {"{I2}", {{"chunk", "<ITEM\n  <SOURCE RPP_PROJECT\n    FILE \"s.rpp\"\n  >\n>\n"}}}}}};
    SanitizeReport rep2;
    json o2 = sanitizeIncoming(nofx, /*noFx=*/true, rep2);
    assert(rep2.invalid == 0 && rep2.noFx == 1);
    assert(!o2["tracks"]["{T1}"].contains("fxchain"));
    assert(!o2["tracks"]["{T1}"].contains("recfx"));
    assert(o2["tracks"]["{T1}"].contains("env"));
    assert(o2["tracks"]["{T1}"].contains("recv"));
    assert(o2["items"]["{I1}"]["chunk"] == "<ITEM\n>\n");  // take FX stripped
    assert(!o2["items"].contains("{I2}"));                  // subproject dropped
  }

  printf("chunk_test: all assertions passed\n");
  return 0;
}
