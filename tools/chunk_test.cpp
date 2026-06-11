// Unit test for the pure state-chunk parsing/splicing helpers in
// extension/src/reamesh.cpp (normalizeChunk, extractTrackBlocks,
// replaceTrackBlocks, chunkFilePaths, isRelativePath). No REAPER needed.
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
    "  <VOLENV2\n"
    "    ACT 1 -1\n"
    "    PT 0 1 0\n"
    "  >\n"
    "  <FXCHAIN\n"
    "    SHOW 2\n"
    "    LASTSEL 0\n"
    "    DOCKED 0\n"
    "    <VST \"VST: ReaEQ\" reaeq.dylib 0\n"
    "      base64data==\n"
    "    >\n"
    "    FLOATPOS 0 0 0 0\n"
    "  >\n"
    "  <ITEM\n"
    "    POSITION 1\n"
    "    SEL 1\n"
    "    <SOURCE WAVE\n"
    "      FILE \"Audio/kick.wav\"\n"
    "    >\n"
    "  >\n"
    ">\n";

int main() {
  // normalizeChunk drops volatile UI/selection lines, keeps the rest.
  std::string norm = normalizeChunk(kTrackChunk);
  assert(norm.find("SEL 1") == std::string::npos);
  assert(norm.find("SHOW 2") == std::string::npos);
  assert(norm.find("LASTSEL") == std::string::npos);
  assert(norm.find("DOCKED") == std::string::npos);
  assert(norm.find("FLOATPOS") == std::string::npos);
  assert(norm.find("NAME \"Drums\"") != std::string::npos);
  assert(norm.find("base64data==") != std::string::npos);
  assert(norm.find("FILE \"Audio/kick.wav\"") != std::string::npos);

  // extractTrackBlocks pulls FXCHAIN and *ENV* blocks, not items.
  TrackBlocks blocks = extractTrackBlocks(norm);
  assert(blocks.fxchain.find("<FXCHAIN") == 0 || blocks.fxchain.find("  <FXCHAIN") == 0);
  assert(blocks.fxchain.find("ReaEQ") != std::string::npos);
  assert(blocks.fxchain.find("POSITION 1") == std::string::npos);
  assert(blocks.env.find("<VOLENV2") != std::string::npos);
  assert(blocks.env.find("PT 0 1 0") != std::string::npos);
  assert(blocks.env.find("ReaEQ") == std::string::npos);

  // Replacing the FX chain keeps everything else byte-identical.
  std::string newFx =
      "  <FXCHAIN\n"
      "    <VST \"VST: ReaComp\" reacomp.dylib 0\n"
      "      othercfg==\n"
      "    >\n"
      "  >\n";
  std::string replaced = replaceTrackBlocks(norm, &newFx, nullptr);
  assert(replaced.find("ReaComp") != std::string::npos);
  assert(replaced.find("ReaEQ") == std::string::npos);
  assert(replaced.find("<VOLENV2") != std::string::npos);     // env untouched
  assert(replaced.find("POSITION 1") != std::string::npos);   // items untouched
  assert(replaced.find("NAME \"Drums\"") != std::string::npos);
  // Round-trip: extracting from the spliced chunk gives back what we put in.
  TrackBlocks reblocks = extractTrackBlocks(replaced);
  assert(reblocks.fxchain == newFx);
  assert(reblocks.env == blocks.env);

  // Removing the FX chain entirely (empty replacement).
  std::string none;
  std::string stripped = replaceTrackBlocks(replaced, &none, nullptr);
  assert(stripped.find("FXCHAIN") == std::string::npos);
  assert(stripped.find("<VOLENV2") != std::string::npos);
  assert(extractTrackBlocks(stripped).fxchain.empty());

  // Replacing envelopes only.
  std::string newEnv =
      "  <PANENV2\n"
      "    PT 0 0 0\n"
      "  >\n";
  std::string envSwapped = replaceTrackBlocks(norm, nullptr, &newEnv);
  assert(envSwapped.find("PANENV2") != std::string::npos);
  assert(envSwapped.find("VOLENV2") == std::string::npos);
  assert(envSwapped.find("ReaEQ") != std::string::npos);  // fxchain untouched

  // Adding a chain to a track that never had one (and no envelopes either).
  std::string bare =
      "<TRACK {2}\n"
      "  NAME \"Bare\"\n"
      "  <ITEM\n"
      "    POSITION 5\n"
      "  >\n"
      ">\n";
  std::string withFx = replaceTrackBlocks(bare, &newFx, nullptr);
  assert(extractTrackBlocks(withFx).fxchain == newFx);
  // The chain must land before the items so REAPER parses it as track FX.
  assert(withFx.find("FXCHAIN") < withFx.find("<ITEM"));

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
