//! Fragmented-MP4 (CMAF) fixups for HLS segments that get joined into one file.

/// Rewrites each top-level `moof`'s `mfhd` sequence number to `next`, `next + 1`, ...
/// Servers that render segments independently (Tella) number every one 1; players that trust the
/// number then read only the first fragment of the joined file, and the video comes out seconds long.
/// The length never changes. Box sizes come from the server: a malformed box ends the scan and the
/// rest of the data is left as it is.
pub fn renumber_fragments(data: &mut [u8], next: &mut u32) {
    let mut off = 0;
    while let Some((kind, header, size)) = box_at(data, off, data.len()) {
        if &kind == b"moof" {
            let end = off + size;
            let mut c = off + header;
            while let Some((child, child_header, child_size)) = box_at(data, c, end) {
                // mfhd: header, 1-byte version, 3-byte flags, 4-byte sequence_number.
                if &child == b"mfhd" && child_size >= child_header + 8 {
                    let at = c + child_header + 4;
                    data[at..at + 4].copy_from_slice(&next.to_be_bytes());
                    *next = next.wrapping_add(1);
                    break;
                }
                c += child_size;
            }
        }
        off += size;
    }
}

/// (type, header length, total size) of the box starting at `at`, if it fits before `end`.
fn box_at(data: &[u8], at: usize, end: usize) -> Option<([u8; 4], usize, usize)> {
    let avail = end.checked_sub(at)?;
    if avail < 8 {
        return None;
    }
    let mut size = u64::from(u32::from_be_bytes(data[at..at + 4].try_into().ok()?));
    let mut header = 8;
    if size == 1 {
        if avail < 16 {
            return None;
        }
        size = u64::from_be_bytes(data[at + 8..at + 16].try_into().ok()?);
        header = 16;
    } else if size == 0 {
        size = avail as u64; // runs to the end
    }
    // Compared as u64 before converting, so a huge size can neither overflow nor index out of bounds.
    if size < header as u64 || size > avail as u64 {
        return None;
    }
    Some((data[at + 4..at + 8].try_into().ok()?, header, size as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    }

    /// moof(mfhd seq=1, traf) + mdat, as Tella serves every segment.
    fn segment() -> Vec<u8> {
        let mfhd = mp4_box(b"mfhd", &[0, 0, 0, 0, 0, 0, 0, 1]);
        let traf = mp4_box(b"traf", &mp4_box(b"tfdt", &[0; 8]));
        let mut s = mp4_box(b"moof", &[mfhd, traf].concat());
        s.extend(mp4_box(b"mdat", b"media"));
        s
    }

    fn sequence(seg: &[u8]) -> u32 {
        u32::from_be_bytes(seg[20..24].try_into().unwrap())
    }

    #[test]
    fn renumbers_fragments_across_segments() {
        let mut next = 1;
        for want in 1..=3 {
            let mut s = segment();
            renumber_fragments(&mut s, &mut next);
            assert_eq!(sequence(&s), want);
            let mut expected = segment();
            expected[20..24].copy_from_slice(&want.to_be_bytes());
            assert_eq!(s, expected, "only the sequence number changes");
        }
        assert_eq!(next, 4);
    }

    #[test]
    fn hostile_box_sizes_leave_data_unchanged() {
        let mut cases = Vec::new();
        for size in [u64::MAX, 0x7fff_ffff_ffff_fff0, 0xffff_fff0] {
            let mut b = 1u32.to_be_bytes().to_vec();
            b.extend_from_slice(b"moof");
            b.extend_from_slice(&size.to_be_bytes());
            b.extend(segment());
            cases.push(b);
        }
        cases.push(segment()[..6].to_vec()); // truncated header
        let mut small = segment();
        small[..4].copy_from_slice(&4u32.to_be_bytes()); // size < header
        cases.push(small);
        let mut short_mfhd = mp4_box(b"moof", &mp4_box(b"mfhd", &[0, 0]));
        short_mfhd.extend(mp4_box(b"mdat", b"x"));
        cases.push(short_mfhd);
        let mut huge32 = segment();
        huge32[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        cases.push(huge32);

        for case in cases {
            let mut data = case.clone();
            let mut next = 7;
            renumber_fragments(&mut data, &mut next);
            assert_eq!(data, case);
            assert_eq!(next, 7);
        }
    }

    #[test]
    fn size_zero_runs_to_the_end() {
        let mut s = segment();
        s[..4].copy_from_slice(&0u32.to_be_bytes()); // the moof claims the rest of the data
        let mut next = 5;
        renumber_fragments(&mut s, &mut next);
        assert_eq!(sequence(&s), 5);
    }
}
