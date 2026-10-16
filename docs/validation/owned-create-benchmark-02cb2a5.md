# Rust-only owned construction benchmark
Retained samples per case: 31. No libxml2 comparison is represented.
Created-node/attribute fields are scenario-derived expectations checked against public live counts; persistent private counters are not exposed.

| case | operation | phase | median ns | median ns/unit | unit | rel MAD | range ns |
|---|---|---|---:|---:|---|---:|---|
| root_plain0 | create | operation | 723167 | 180.8 | operation | 1.6% | 703208..803917 |
| root_plain0 | create | traverse | 27625 | 6.9 | document | 0.5% | 27334..29709 |
| root_plain0 | create | serialize | 483375 | 120.8 | document | 0.2% | 460333..514709 |
| root_plain0 | create | drop | 73375 | 18.3 | document | 0.5% | 72833..76417 |
| root_plain2 | create | operation | 680833 | 340.4 | operation | 0.8% | 667333..716917 |
| root_plain2 | create | traverse | 49042 | 24.5 | document | 0.5% | 48708..58792 |
| root_plain2 | create | serialize | 944666 | 472.3 | document | 0.5% | 933167..985792 |
| root_plain2 | create | drop | 59542 | 29.8 | document | 0.4% | 59084..62500 |
| append_default | append | operation | 607625 | 303.8 | operation | 1.5% | 590417..635083 |
| append_default | append | traverse | 124833 | 62.4 | document | 0.4% | 123000..131791 |
| append_default | append | serialize | 2009833 | 1004.9 | document | 0.5% | 1992709..2102042 |
| append_default | append | drop | 66875 | 33.4 | document | 0.8% | 66291..73041 |
| append_reuse_prefix | append | operation | 540125 | 270.1 | operation | 0.2% | 538375..567375 |
| append_reuse_prefix | append | traverse | 134875 | 67.4 | document | 0.3% | 133708..141041 |
| append_reuse_prefix | append | serialize | 2404250 | 1202.1 | document | 0.7% | 2341875..2525333 |
| append_reuse_prefix | append | drop | 68667 | 34.3 | document | 1.5% | 67166..77958 |
| append_generate_prefix | append | operation | 1062917 | 531.5 | operation | 1.0% | 1042250..1122333 |
| append_generate_prefix | append | traverse | 140584 | 70.3 | document | 0.3% | 139834..151833 |
| append_generate_prefix | append | serialize | 2373583 | 1186.8 | document | 1.2% | 2296708..2453583 |
| append_generate_prefix | append | drop | 67417 | 33.7 | document | 0.9% | 66375..72583 |
| append_batch0 | append | operation | 670917 | 163.8 | operation | 1.2% | 658166..702958 |
| append_batch0 | append | traverse | 22917 | 179.0 | document | 0.5% | 22583..25750 |
| append_batch0 | append | serialize | 344375 | 2690.4 | document | 0.3% | 340375..356708 |
| append_batch0 | append | drop | 4292 | 33.5 | document | 1.9% | 4083..7166 |
| append_batch4 | append | operation | 1794334 | 438.1 | operation | 0.6% | 1766334..1858625 |
| append_batch4 | append | traverse | 118959 | 929.4 | document | 0.1% | 118666..126500 |
| append_batch4 | append | serialize | 2512375 | 19627.9 | document | 0.3% | 2481458..2634834 |
| append_batch4 | append | drop | 6667 | 52.1 | document | 1.2% | 6459..9542 |
| unicode | create | operation | 577000 | 288.5 | operation | 0.9% | 566875..596000 |
| unicode | create | traverse | 33834 | 16.9 | document | 0.5% | 33500..34833 |
| unicode | create | serialize | 707042 | 353.5 | document | 0.6% | 699583..725083 |
| unicode | create | drop | 56500 | 28.2 | document | 0.4% | 56125..63125 |
| long_prefix | append | operation | 156292 | 2442.1 | operation | 3.0% | 146833..178250 |
| long_prefix | append | traverse | 977959 | 15280.6 | document | 0.0% | 977792..1056417 |
| long_prefix | append | serialize | 2341750 | 36589.8 | document | 0.1% | 2332750..2386667 |
| long_prefix | append | drop | 4083 | 63.8 | document | 3.1% | 3792..6833 |

# Public layout and accounting identity
| case | iterations | ops/doc | expected created nodes/doc | expected created attrs/doc | final nodes/doc | final attrs/doc | output bytes/doc | document bytes | node-id bytes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| root_plain0 | 4000 | 1 | 1 | 0 | 2 | 0 | 4 | 168 | 16 |
| root_plain2 | 2000 | 1 | 1 | 2 | 2 | 2 | 25 | 168 | 16 |
| append_default | 2000 | 1 | 1 | 0 | 3 | 1 | 29 | 168 | 16 |
| append_reuse_prefix | 2000 | 1 | 1 | 1 | 3 | 2 | 39 | 168 | 16 |
| append_generate_prefix | 2000 | 1 | 1 | 2 | 3 | 2 | 43 | 168 | 16 |
| append_batch0 | 128 | 32 | 32 | 0 | 34 | 0 | 135 | 168 | 16 |
| append_batch4 | 128 | 32 | 32 | 128 | 34 | 128 | 1031 | 168 | 16 |
| unicode | 2000 | 1 | 1 | 1 | 2 | 1 | 18 | 168 | 16 |
| long_prefix | 64 | 1 | 1 | 1 | 3 | 2 | 8236 | 168 | 16 |
