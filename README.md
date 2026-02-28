# Petroglyph Meg file library

MEGA files are a format used by Petroglyph for various games including Star Wars: Empire
at War, Universe at War: Earth Assault, Guardians of Graxia, Rise of Immortals, Grey Goo,
and Great War: Western Front.

This repo provides a Rust Library and other tools for working with them.

The parsing and encoding is based on the format descriptions provided
[here](https://modtools.petrolution.net/docs/MegFileFormat) by some of the Modders who
first enabled Empire at War modding.

## Why?

Q: *You're like 20 years late to this party. Why make this now?*

Mostly for fun. Also because the [MEGA file
editor](https://github.com/GlyphXTools/meg-editor/) produced by Mike Lankamp is a C# GUI
intended for Windows and I'm a Linux dev who likes CLI tools.

There are some tweaks I like to apply to *all* projectiles in Empire at War (e.g. making
*all* lasers faster), and I tend to write scripts to do that rather than hand editing the
XML config files. By making this a library, I can write a script that directly extracts
and edits files from the game's MEGA files without needing to provide it a pre-extracted
bundle of XML "source" files to work with.
