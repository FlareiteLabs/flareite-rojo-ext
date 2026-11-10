# Rojo

Roblox development with [Rojo](https://rojo.space): serve a project to Roblox Studio, build it,
keep the sourcemap luau-lsp reads, see the project's instances, and check project files against
Roblox's API. With [Rokit](https://github.com/rojo-rbx/rokit) and
[Wally](https://wally.run) for tools and packages.

It starts in folders with a Rojo project (`*.project.json`), a `rokit.toml`, or a `wally.toml`.
Install the **Luau** extension too, for highlighting and luau-lsp.

## Setting up

Install Rojo with Rokit (`rokit add rojo-rbx/rojo` in a project, or `rokit add --global
rojo-rbx/rojo`), or from its [releases](https://github.com/rojo-rbx/rojo/releases). If it's
missing, the extension offers to add it with Rokit. To use a Rojo that isn't on the `PATH`, set
its path in `settings.toml`:

```toml
[extensions.programs]
"flareite.rojo.rojo" = "C:/tools/rojo.exe"
```

The extension runs programs only in folders you trust.

## Serving to Studio

**Rojo** in the status bar serves the project (or stops serving), and shows the port Rojo listens
on. In Studio, connect from Rojo's plugin. The first time you serve with a version of Rojo, the
extension offers to install its Studio plugin (`rojo plugin install`), which should be the same
version; **Rojo: Install Roblox Studio Plugin** installs it any time.

Rojo's output is in **View: Output**, under Rojo. Its errors, such as a file it can't read while
syncing, show as notifications.

With several projects in the folder, the **Project** setting picks the one served from the status
bar; the Projects view serves any of them.

## The Rojo view

The **Rojo** entry in the activity bar has two sections:

- **Projects**: the folder's project files. Serve, stop, and build from the row's buttons; its
  menu has **Build and Open in Roblox Studio**, **Upload Project to Roblox**, **Syncback from a
  Place File**, **Format Project File**, and opening the file. The section's menu installs Rojo,
  the Studio plugin, Rokit's tools, and Wally's packages.
- **Instances**: the instances the project makes, from the sourcemap. Click one to open its file.

## Sourcemaps

luau-lsp resolves `require(script.Parent.Module)` and instance paths with a `sourcemap.json`.
The extension keeps it up to date with `rojo sourcemap --watch` from the Project setting's
project (or the first one). Turn off **Sourcemap** to make it yourself, and **Sourcemap Include
Non Scripts** to list only scripts. **Rojo: Regenerate Sourcemap** starts Rojo again.

## The explorer

Files are marked with the instances they become: **SS** for a Script, **LS** for a LocalScript (or
a client Script), **MS** for a ModuleScript, and an `init` file's folder like the file. Hover a
file for its class and path, such as `ModuleScript game.ReplicatedStorage.Shared.Hello`. Turn off
**Explorer Marks** to hide them.

## Project files

`*.project.json`, `*.meta.json`, and `*.model.json` files are checked as you edit them, and
complete: class names, and under `$properties` (or `properties`) the class's properties, its
superclasses' included, with their types, enums' items, and Roblox's documentation on hover. A
service's properties complete by its name under a DataModel.

The classes come from Roblox's API ([Roblox-Client-Tracker](https://github.com/MaximumADHD/Roblox-Client-Tracker)),
and the documentation from [luau-lsp](https://luau-lsp.pages.dev); Flareite downloads both and
gets them again once they're a day old. Without the network permission, the files' structure is
still checked.

## Building, uploading, and syncback

- **Build Project** asks where to build (`<name>.rbxl` for a place, `<name>.rbxm` for a model,
  remembered per project). **Build and Open in Roblox Studio** opens the place after.
- **Upload Project to Roblox** publishes a place with Open Cloud: it asks for the experience's
  universe ID and the place ID (remembered per project), and an API key with place publishing
  access from [create.roblox.com/credentials](https://create.roblox.com/credentials). The key is
  kept in the system's credential store (Windows Credential Manager, the macOS Keychain, or the
  Secret Service on Linux); **Forget Open Cloud API Key** removes it.
- **Syncback from a Place File** rewrites the project's files to match a `.rbxl` place (Rojo 7.5
  or later), for moving an existing game to Rojo. It asks first: commit your work before.
- **New Project** runs `rojo init` for a place, a model, or a plugin.

## Rokit and Wally

`rokit.toml` and `wally.toml` are checked as you edit them. **Rokit: Install Rokit Tools**
installs the tools `rokit.toml` lists (and trusts them). When `wally.toml` changes, the extension
offers to run `wally install` (turn off **Wally Prompt** to stop it), as does **Wally: Install
Wally Packages**; then, if `wally-package-types` is installed, it types the packages for luau-lsp.
