// Unit test for the pure state-chunk parsing/splicing helpers in
// extension/src/reamesh.cpp (normalizeChunk, extractTrackBlocks,
// replaceTrackBlocks, transformRecvUnits, chunkFilePaths, isRelativePath).
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
    "    >\n"
    "    FLOATPOS 0 0 0 0\n"
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
  assert(norm.find("ARM ") == std::string::npos);
  assert(norm.find("VIS ") == std::string::npos);
  assert(norm.find("ACT 1 -1") != std::string::npos);   // envelope active state stays
  assert(norm.find("BYPASS 0 0 0") != std::string::npos);
  assert(norm.find("NAME \"Drums\"") != std::string::npos);
  assert(norm.find("base64data==") != std::string::npos);
  assert(norm.find("FILE \"Audio/kick.wav\"") != std::string::npos);

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

  printf("chunk_test: all assertions passed\n");
  return 0;
}
