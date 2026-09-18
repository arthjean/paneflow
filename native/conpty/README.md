# Windows PTY runtime

Paneflow embeds the Microsoft `conpty.dll` and `OpenConsole.exe` pair from
the stable [Microsoft.Windows.Console.ConPTY NuGet package](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY/1.24.260710001).
The package supports Windows 10 build 17763 and later. The currently shipping
Windows target is x86_64 MSVC, matching the libghostty archive manifest.

Run `scripts/fetch-conpty.ps1` with PowerShell 7 before building on Windows.
`scripts/dev.ps1` and the CI native-dependency action run it automatically.
The script verifies both the package and each extracted binary against
`manifest.json`. Cargo verifies the binary hashes again before embedding them;
it does not download dependencies from the build script. The binaries stay in
the ignored `prebuilt/` directory. Updating the pin requires updating the
package URL, version and all hashes together.

The shared `paneflow_host::pty::open` entry point installs these embedded bytes
under Paneflow's `cache/conpty/<version>/<target>/` on a worker thread. Installation
is locked across processes, checks existing bytes and replaces incomplete files
atomically. The runtime loads the DLL by absolute path with a restricted dependency
search path before `portable-pty` resolves its ConPTY functions. The DLL stays
loaded for the process lifetime. Missing or unusable embedded runtimes produce a
session startup error. No Windows Terminal installation or system update is needed.

This avoids the older system ConPTY VT renderer, which can emit a DEC 2026 end
marker before restoring the cursor. Modern OpenConsole preserves that ordering.
The regression executable `cargo test -p paneflow-host --test conpty --locked`
runs a real child in the production PTY and checks the libghostty cursor at the
exact end of the synchronized update, regardless of how pipe reads are split.
It does not exercise GPU presentation.

The upstream sources are [microsoft/terminal](https://github.com/microsoft/terminal),
particularly `src/host/_stream.cpp` (`WriteCharsVT`) and
`src/winconpty/winconpty.cpp` (the adjacent OpenConsole host).
Microsoft licenses this package under MIT. [LICENSE](LICENSE) is embedded and
installed alongside both binaries.
