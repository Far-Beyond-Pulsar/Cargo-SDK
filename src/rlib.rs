//! Reading and rewriting rlibs.
//!
//! An rlib is an `ar` archive: a symbol table, the crate's metadata (`lib.rmeta`, an object file that wraps the
//! same bytes as the standalone `.rmeta`), a small link-info member, and the machine code. For a crate that
//! lives inside a linked dylib rustc reads only the metadata, so the code is dead weight: [`strip`] drops it.

const MAGIC: &[u8; 8] = b"!<arch>\n";
const HEADER: usize = 60;

/// Members kept by [`strip`].
const KEPT: &[&str] = &["lib.rmeta", "lib.rmeta-link"];

/// One archive member: its header (exactly as written) and its data.
struct Member<'a> {
    header: &'a [u8],
    name: String,
    data: &'a [u8],
}

fn members(archive: &[u8]) -> Result<Vec<Member<'_>>, String> {
    if archive.len() < 8 || &archive[..8] != MAGIC {
        return Err("not an `ar` archive".into());
    }
    let mut at = 8;
    let mut out = Vec::new();
    while at + HEADER <= archive.len() {
        let header = &archive[at..at + HEADER];
        let field = String::from_utf8_lossy(&header[..16]).trim_end().to_owned();
        let size: usize = String::from_utf8_lossy(&header[48..58])
            .trim()
            .parse()
            .map_err(|_| format!("bad member size in `{field}`"))?;
        let start = at + HEADER;
        let end = start + size;
        if end > archive.len() {
            return Err(format!("member `{field}` runs past the end of the archive"));
        }
        let data = &archive[start..end];
        // BSD archives (macOS) put the name in the first bytes of the data: `#1/<length>`. GNU and
        // Windows archives end a short name with `/`. Either way the member is named without it.
        let name = match field
            .strip_prefix("#1/")
            .and_then(|n| n.parse::<usize>().ok())
        {
            Some(len) if len <= data.len() => String::from_utf8_lossy(&data[..len])
                .trim_end_matches('\0')
                .to_owned(),
            _ => field,
        };
        out.push(Member {
            header,
            name: name.trim_end_matches('/').to_owned(),
            data,
        });
        at = end + (size % 2);
    }
    Ok(out)
}

/// The rlib with only its metadata members: what rustc needs of a crate whose code is in a linked dylib.
pub fn strip(rlib: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(rlib.len() / 2);
    out.extend_from_slice(MAGIC);
    let mut kept_meta = false;
    for member in members(rlib)? {
        if !KEPT.contains(&member.name.as_str()) {
            continue;
        }
        kept_meta |= member.name == "lib.rmeta";
        out.extend_from_slice(member.header);
        out.extend_from_slice(member.data);
        if member.data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    if !kept_meta {
        return Err("no `lib.rmeta` member".into());
    }
    Ok(out)
}

/// Where `needle` (the plain `.rmeta` file) starts inside `haystack` (the stripped rlib).
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    // The needle's first bytes are its magic and version; search for a prefix, then confirm the rest.
    let prefix = &needle[..needle.len().min(64)];
    let first = prefix[0];
    let mut from = 0;
    while let Some(i) = haystack[from..].iter().position(|&b| b == first) {
        let at = from + i;
        if haystack[at..].starts_with(prefix) && haystack[at..].starts_with(needle) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for (name, data) in members {
            let header = format!(
                "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                name,
                "0",
                "0",
                "0",
                "644",
                data.len()
            );
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(data);
            if data.len() % 2 == 1 {
                out.push(b'\n');
            }
        }
        out
    }

    #[test]
    fn strip_keeps_only_the_metadata_members() {
        let rlib = archive(&[
            ("/", b"symbols"),
            ("lib.rmeta/", b"rust-metadata!"),
            ("code.o/", b"machine code"),
        ]);
        let stripped = strip(&rlib).unwrap();
        let names: Vec<_> = members(&stripped)
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, ["lib.rmeta"]);
        assert!(stripped.len() < rlib.len());
    }

    #[test]
    fn the_metadata_is_found_where_it_was_written() {
        let rlib = archive(&[("lib.rmeta/", b"..header..rust-metadata!..tail")]);
        let at = find(&rlib, b"rust-metadata!").unwrap();
        assert_eq!(&rlib[at..at + 14], b"rust-metadata!");
    }

    #[test]
    fn a_bsd_archive_names_its_members_in_their_data() {
        // macOS: the name field is `#1/<length>` and the name is the first bytes of the data.
        let name = b"lib.rmeta\0\0\0";
        let payload = b"rust-metadata!";
        let size = name.len() + payload.len();
        let mut rlib = MAGIC.to_vec();
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            "#1/12", "0", "0", "0", "644", size
        );
        rlib.extend_from_slice(header.as_bytes());
        rlib.extend_from_slice(name);
        rlib.extend_from_slice(payload);
        let stripped = strip(&rlib).unwrap();
        assert_eq!(stripped, rlib);
        assert!(find(&stripped, payload).is_some());
    }

    #[test]
    fn an_archive_without_metadata_is_refused() {
        assert!(strip(&archive(&[("code.o/", b"x")])).is_err());
    }
}
