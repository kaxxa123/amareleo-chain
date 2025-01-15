# Code Migration Notes

As we migrate snarkos code into amareleo-chain, we keep notes on key changes. We also create basic code documentation that might help future restructuring. 

* When it comes to describing code, we are not interested in details. The goal is to sumarize the purpose and identify logically related code.

* Documented code blocks may cover an entire sub-crate or indvidual modules. Hence we refer to the code using relative paths rather than crate names.

* __No cross-dependencies__ - This label highlights how the sub-crate/module does not depend on other sub-crates/modules within snarkos (except for the `snarkos-lite-node-metrics` sub-crate).

* The main executable is renamed from `snarkos` to `amareleo-chain`.

* All snarkos sub-crates are renamed from `snarkos-*` to `snarkos-lite-*` even if the code is otherwise identical. 

* The `amareleo-chain` ledger files/folders where renamed as follows:
    | snarkos                    | amareleo-chain              |
    |----------------------------|-----------------------------|
    | `.current-proposal-cache*` | `.amareleo-proposal-cache*` |
    | `.ledger-*`                | `.amareleo-ledger-*`        |
    | `/tmp/snakros.log`         | `/tmp/amareleo-chain.log`   |

* Just like snarkos, amareleo-chain relies on caching to disk the genesis block. Both use the same filename. The first time a node is run this will do extra work that will speed up launching other node instances. The name is deterministic and is currently: <BR />
`/tmp/15983110333109949993277199043008208330432506493933185651419933655310578437field`


<BR />

## Code Encapsulation Issues

There are some code encapsulation issues within `snarkos` and `snarkvm`. Specifically:

* The minimum committee size requirements are hardcoded in both `snarkos` and `snarkvm`. 
For this reason, at this stage we mock having 4 committee members in a single node. 
An alternative approach would be to remove/replace the consensus altogether.

* The ledger directory path is returned by [aleo-std](https://github.com/ProvableHQ/aleo-std) | `aleo_ledger_dir()`.
This is invoked directly both from `snarkos` and `snarkvm`. <BR />
`amareleo-chain` aims to rename the ledger folder. We achive this by setting `StorageMode::Custom` with our custom 
ledger folder. This `StorageMode` instructs `aleo_ledger_dir()` to use our custom path.


<BR />

## Notes

* `/node/metrics` - Prometheus/Grafana Metrics. <BR />
    __No cross-dependencies__

* `/node/sync/locators` - Recent block handling object. <BR />
    __No cross-dependencies__

* `/node/bft/events` - Raw encoding/decoding of messages (events) such as 
    batch proposals, batch signatures, etc.  <BR />
    __Dependent__ on `BlockLocators`, `Metrics`

*  `/node/bft/src/helpers/channels.rs` - Message (events) passing channels communication 
    between threads.  <BR />
    __Dependent__ on `Events`, `BlockLocators`

*  `/node/bft/src/helpers/proposal.rs` - Proposal object composed of 
    &lt;header, transmissions, signatures&gt; <BR />
    __No cross-dependencies__, except for tests (removed).

*  `/node/bft/src/helpers/signed_proposals.rs` -  A HashMap of recently signed proposals 
    where each maps `address => (round, batch_id, signature)`  <BR />
    __No cross-dependencies__

*  `/node/bft/ledger-service` - Implements structs exposing the `LedgerService`
    trait. The most useful is the `CoreLedgerService` which encapsulates the
    `LedgerService` implementation for validator nodes. 

    The struct includes:
    1. snarkvm::Ledger, 
    2. Cache of committees organized by round
    3. Latest committee leader
    4. Shutdown flag.
    
    __No cross-dependencies__

* `/node/bft/storage-service` - Implements memory pool storage structs exposing the 
    `StorageService` trait. `BFTMemoryService` provides in-memory storage. 
    `BFTPersistentStorage` persists to a rocksdb within the ledger folder. <BR />
    __No cross-dependencies__


* `/node/bft/src/helpers/storage.rs` - Implements complete memory pool storage in 
    `Storage` and `StorageInner`. This includes:
    1. `LedgerService`
    2. Block Hieght, Round Number
    3. Last garabage collected round.
    4. Maxium number of rounds stored.
    5. Map (round_num => List of (certificate_id, batch_id, signer_addr))
    6. Map (certificate_id => certificate)
    7. Map (batch_id => round_num)
    8. `StorageService`

    __Dependent__ on `LedgerService`, `StorageService`


* `/node/bft/src/helpers/ready.rs` - Implements a map of transmissions that have completed processing `transmission_id => transmission`. Where a transmission can be one of: <BR />
    * Ratification 
    * Solution
    * Transaction

    __No cross-dependencies__


* `/node/bft/src/worker.rs` - Implements the Worker object responsible for processing transmissions. 

    In snarkos this would send out requests/responses for transmissions to peers. In snarkos the receiving side would be the `Sync` object. The amareleo-chain Worker is stripped down from the network communcation (gateway) component.

    __Dependent__ on `LedgerService`, `StorageService`, `Ready`, `Proposal`


* `/node/sync/src/block_sync.rs` - Implements `BlockSyncMode` and `BlockSync`. `BlockSync` is reponsible for syncing Blocks across peers. This is largely unecessary for amareleo-chain. At this stage we are keeping a minimal implementation with no network access. Later we might remove this altogether.

    __Dependent__ on `LedgerService`, `BlockLocators`


* `/node/bft/src/sync/mod.rs` - Implements the `Sync` module which is resposnbile to sync the ledger from storage and keeping the ledger in a synced state. In snarkos, `Sync` receives messages from other peer `Worker` instances. However in amareleo-chain all that communication was removed. In snarkos, `Sync` employes `BlockSync` for exchanging blocks with peers. In amareleo-chain this `BlockSync` dependency was removed with some of its functianality merged into `Sync`.

    __Dependent__ on `Storage`, `LedgerService`, `BlockLocators`


* `/node/bft/src/primary.rs` - Implements the `Primary` object, a DAG-based Narwhal mempool manager. This object has been greatly modified to allow a single object to create proposal certificates for itself and other fake validators.

    __Dependent__ on `Proposal`, `Storage`, `Sync`,  `Worker`, `LedgerService`


* `/node/bft/src/helpers/dag.rs` - Implements the `DAG` object an in-memory DAG for Narwhal to store its rounds.

    __No cross-dependencies__


* `/node/bft/src/bft.rs` - Implements the `BFT` object bringing together `Primary` and `DAG` largely encapsulating Narwhal.

    __Dependent__ on `Primary`, `DAG`


* `/node/consensus/src/lib.rs` -  Implements the `Consensus` object adding the Bullshark consensus to Narhwal.

    __Dependent__ on `BFT`, `Ledger`, `Storage`


* `/node/rest` - Implements rest server.

    __Dependent__ on `Consensus`
