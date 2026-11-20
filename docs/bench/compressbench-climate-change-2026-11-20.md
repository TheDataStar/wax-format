# compressbench — /tmp/claude-0/bench/cc-nons.wax

entries with content: 3992 (text 3889, media 103); text raw 30326182 bytes; media raw 2304645 bytes
text entry size: p10 4197 · p50 6533 · p90 11080 · p99 25512 · max 347286 bytes; decode timings best of 3 passes, single thread

## 1. zstd level, text entries (one frame per entry)

| codec | bytes | + dict | ratio (incl. dict) | vs zstd-3+dict | encode MB/s | decode MB/s | mean µs | p50 µs | p99 µs |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| zstd-1 | 10349369 | 0 | 0.3413 | +181.2% | 285.7 | 890 | 8.8 | 7.8 | 21.5 |
| zstd-3 | 10108438 | 0 | 0.3333 | +174.7% | 230.4 | 893 | 8.7 | 7.8 | 21.5 |
| zstd-6 | 9789088 | 0 | 0.3228 | +166.0% | 81.9 | 917 | 8.5 | 7.6 | 20.9 |
| zstd-9 | 9767797 | 0 | 0.3221 | +165.4% | 40.6 | 891 | 8.8 | 7.8 | 21.0 |
| zstd-12 | 9730657 | 0 | 0.3209 | +164.4% | 16.6 | 754 | 10.3 | 9.1 | 26.1 |
| zstd-15 | 9674254 | 0 | 0.3190 | +162.9% | 8.5 | 867 | 9.0 | 8.0 | 22.4 |
| zstd-17 | 9660531 | 0 | 0.3186 | +162.5% | 4.7 | 852 | 9.2 | 8.1 | 22.2 |
| zstd-19 | 9657844 | 0 | 0.3185 | +162.4% | 3.9 | 849 | 9.2 | 8.2 | 22.4 |
| zstd-22 | 9657641 | 0 | 0.3185 | +162.4% | 4.0 | 859 | 9.1 | 8.1 | 22.0 |
| zstd-1 + 110 KiB dict | 4254020 | 112640 | 0.1440 | +18.7% | 491.9 | 1572 | 5.0 | 4.1 | 17.8 |
| zstd-3 + 110 KiB dict | 3567256 | 112640 | 0.1213 | +0.0% | 410.4 | 1645 | 4.7 | 3.6 | 17.7 |
| zstd-6 + 110 KiB dict | 3337999 | 112640 | 0.1138 | -6.2% | 119.3 | 1111 | 7.0 | 5.3 | 28.3 |
| zstd-9 + 110 KiB dict | 3151939 | 112640 | 0.1076 | -11.3% | 70.5 | 1726 | 4.5 | 3.3 | 18.5 |
| zstd-12 + 110 KiB dict | 3062441 | 112640 | 0.1047 | -13.7% | 39.2 | 1758 | 4.4 | 3.3 | 18.6 |
| zstd-15 + 110 KiB dict | 2974821 | 112640 | 0.1018 | -16.1% | 12.4 | 1754 | 4.4 | 3.3 | 18.9 |
| zstd-17 + 110 KiB dict | 2963171 | 112640 | 0.1014 | -16.4% | 6.3 | 1657 | 4.7 | 3.5 | 20.5 |
| zstd-19 + 110 KiB dict | 2963503 | 112640 | 0.1014 | -16.4% | 6.1 | 1720 | 4.5 | 3.4 | 19.2 |
| zstd-22 + 110 KiB dict | 2956568 | 112640 | 0.1012 | -16.6% | 1.7 | 1713 | 4.6 | 3.4 | 19.6 |

## 2. dictionary size (trained by the zim2wax rule), text entries

| codec | bytes | + dict | ratio (incl. dict) | vs zstd-3+dict | encode MB/s | decode MB/s | mean µs | p50 µs | p99 µs |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| zstd-3 + 16 KiB dict | 4360271 | 16384 | 0.1443 | +18.9% | 343.1 | 1559 | 5.0 | 4.2 | 19.4 |
| zstd-3 + 32 KiB dict | 4047244 | 32768 | 0.1345 | +10.9% | 416.0 | 1563 | 5.0 | 4.0 | 17.2 |
| zstd-3 + 64 KiB dict | 3768202 | 65536 | 0.1264 | +4.2% | 284.2 | 1610 | 4.8 | 3.7 | 18.3 |
| zstd-3 + 110 KiB dict | 3567256 | 112640 | 0.1213 | +0.0% | 291.9 | 1411 | 5.5 | 4.1 | 22.6 |
| zstd-3 + 256 KiB dict | 3360068 | 262144 | 0.1194 | -1.6% | 390.0 | 1854 | 4.2 | 3.2 | 16.2 |
| zstd-3 + 512 KiB dict | 3210485 | 524288 | 0.1232 | +1.5% | 276.6 | 1802 | 4.3 | 3.3 | 16.9 |
| zstd-19 + 16 KiB dict | 3903191 | 16384 | 0.1292 | +6.5% | 6.4 | 1491 | 5.2 | 4.1 | 18.1 |
| zstd-19 + 32 KiB dict | 3536599 | 32768 | 0.1177 | -3.0% | 6.5 | 1570 | 5.0 | 3.9 | 17.9 |
| zstd-19 + 64 KiB dict | 3191760 | 65536 | 0.1074 | -11.5% | 6.2 | 1759 | 4.4 | 3.3 | 17.9 |
| zstd-19 + 110 KiB dict | 2963503 | 112640 | 0.1014 | -16.4% | 6.5 | 1759 | 4.4 | 3.3 | 19.5 |
| zstd-19 + 256 KiB dict | 2712429 | 262144 | 0.0981 | -19.2% | 4.9 | 2096 | 3.7 | 2.7 | 16.8 |
| zstd-19 + 512 KiB dict | 2496200 | 524288 | 0.0996 | -17.9% | 4.2 | 2076 | 3.8 | 2.8 | 16.8 |

## 2b. dictionary size, held out (train on even entries, measure odd), text entries

| codec | bytes | + dict | ratio (incl. dict) | vs zstd-3+dict | encode MB/s | decode MB/s | mean µs | p50 µs | p99 µs |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| zstd-3 + 16 KiB dict | 2197741 | 16384 | 0.1452 | +15.0% | 338.8 | 1592 | 4.9 | 4.1 | 19.6 |
| zstd-3 + 32 KiB dict | 2038874 | 32768 | 0.1359 | +7.6% | 328.2 | 1045 | 7.5 | 5.8 | 28.7 |
| zstd-3 + 64 KiB dict | 1883163 | 65536 | 0.1278 | +1.2% | 425.8 | 1667 | 4.7 | 3.5 | 18.5 |
| zstd-3 + 110 KiB dict | 1813005 | 112640 | 0.1263 | +0.0% | 458.9 | 1719 | 4.6 | 3.4 | 18.4 |
| zstd-3 + 256 KiB dict | 1728442 | 262144 | 0.1306 | +3.4% | 329.0 | 1747 | 4.5 | 3.3 | 19.1 |
| zstd-3 + 512 KiB dict | 1676090 | 524288 | 0.1443 | +14.3% | 313.1 | 1885 | 4.2 | 3.1 | 18.2 |
| zstd-19 + 16 KiB dict | 1968466 | 16384 | 0.1302 | +3.1% | 7.3 | 1567 | 5.0 | 3.9 | 20.4 |
| zstd-19 + 32 KiB dict | 1775163 | 32768 | 0.1186 | -6.1% | 5.8 | 1468 | 5.3 | 4.0 | 22.7 |
| zstd-19 + 64 KiB dict | 1609332 | 65536 | 0.1099 | -13.0% | 6.6 | 1781 | 4.4 | 3.2 | 18.8 |
| zstd-19 + 110 KiB dict | 1502448 | 112640 | 0.1059 | -16.1% | 6.1 | 1856 | 4.2 | 3.0 | 18.4 |
| zstd-19 + 256 KiB dict | 1394305 | 262144 | 0.1087 | -14.0% | 5.0 | 2000 | 3.9 | 2.8 | 18.4 |
| zstd-19 + 512 KiB dict | 1314921 | 524288 | 0.1206 | -4.5% | 4.2 | 2249 | 3.5 | 2.4 | 16.6 |

(in 2b, the vs column is against zstd-3 + 110 KiB trained on the even half)

## 3. other codecs, per entry, text entries (no trained dictionary)

brotli carries its own built-in web-text dictionary; xz has none.

| codec | bytes | + dict | ratio (incl. dict) | vs zstd-3+dict | encode MB/s | decode MB/s | mean µs | p50 µs | p99 µs |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| brotli q5 w22 | 8626192 | 0 | 0.2844 | +134.4% | 14.2 | 182 | 42.9 | 38.8 | 97.8 |
| brotli q9 w22 | 8615726 | 0 | 0.2841 | +134.1% | 2.2 | 183 | 42.6 | 38.6 | 95.3 |
| brotli q11 w22 | 7420701 | 0 | 0.2447 | +101.7% | 0.7 | 181 | 43.1 | 39.0 | 100.4 |
| xz -6 | 9898224 | 0 | 0.3264 | +169.0% | 3.5 | 53 | 147.5 | 133.8 | 363.3 |
| xz -9e | 9898372 | 0 | 0.3264 | +169.0% | 0.9 | 47 | 167.2 | 151.6 | 419.5 |

## 4. grouping small text entries into blocks (large entries stay one frame each)

`read µs` is the cost to reach one small article: its whole block must be decoded.
Delta cost is the ADR-0001 concern: one changed article re-ships its whole block.

| level | small = under | block | frames | bytes + dict | vs zstd-3+dict | read µs (small article, mean) | read µs (per-entry, same level) |
|---|---:|---:|---:|---:|---:|---:|---:|
| 3 | 4 KiB | 64 KiB | 3587 | 3664765 | -0.4% | 22.0 | 4.6 |
| 3 | 4 KiB | 256 KiB | 3575 | 3663269 | -0.5% | 74.8 | 4.6 |
| 3 | 4 KiB | 1024 KiB | 3572 | 3662187 | -0.5% | 214.2 | 4.6 |
| 3 | 16 KiB | 64 KiB | 544 | 3472967 | -5.6% | 34.3 | 4.6 |
| 3 | 16 KiB | 256 KiB | 275 | 3404876 | -7.5% | 112.8 | 4.6 |
| 3 | 16 KiB | 1024 KiB | 203 | 3344241 | -9.1% | 379.5 | 4.6 |
| 3 | all | 64 KiB | 419 | 3444584 | -6.4% | 37.3 | 4.6 |
| 3 | all | 256 KiB | 113 | 3355473 | -8.8% | 122.7 | 4.6 |
| 3 | all | 1024 KiB | 29 | 3273101 | -11.1% | 406.2 | 4.6 |
| 19 | 4 KiB | 64 KiB | 3587 | 3062007 | -16.8% | 22.9 | 4.3 |
| 19 | 4 KiB | 256 KiB | 3575 | 3062238 | -16.8% | 67.7 | 4.3 |
| 19 | 4 KiB | 1024 KiB | 3572 | 3061988 | -16.8% | 190.3 | 4.3 |
| 19 | 16 KiB | 64 KiB | 544 | 2896350 | -21.3% | 31.5 | 4.3 |
| 19 | 16 KiB | 256 KiB | 275 | 2878026 | -21.8% | 95.8 | 4.3 |
| 19 | 16 KiB | 1024 KiB | 203 | 2851681 | -22.5% | 370.4 | 4.3 |
| 19 | all | 64 KiB | 419 | 2871740 | -22.0% | 34.3 | 4.3 |
| 19 | all | 256 KiB | 113 | 2846321 | -22.7% | 106.6 | 4.3 |
| 19 | all | 1024 KiB | 29 | 2806104 | -23.7% | 491.7 | 4.3 |

## 5. media (stored verbatim today), by mimetype

| mimetype | entries | raw bytes | zstd-3 | zstd-19 | brotli q11 w22 | xz -9e |
|---|---:|---:|---:|---:|---:|---:|
| image/png | 3 | 20637 | +0.00% | +0.00% | +0.00% | +0.00% |
| image/webp | 100 | 2284008 | -0.01% | -0.03% | -0.03% | -0.01% |

(media cells: size change if each entry keeps the smaller of codec output and verbatim)

## 6. plain-HTTP serve path, per text entry (decode stored frame, then optional gzip for the wire)

| stored as | decode µs (mean) | + gzip-1 µs | + gzip-6 µs | wire bytes identity | wire bytes gzip-1 | wire bytes gzip-6 |
|---|---:|---:|---:|---:|---:|---:|
| zstd-3 + 110 KiB dict | 4.6 | 64.0 | 141.0 | 30326182 | 11209872 | 9859820 |
| zstd-19 + 110 KiB dict | 4.3 | 62.2 | 121.7 | 30326182 | 11209872 | 9859820 |
