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
    | `snakros.log`              | `amareleo-chain.log`        |

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

*  `/node/bft/ledger-service` - Implements a number of structs exposing the `LedgerService`
    trait. The most useful is the `CoreLedgerService` which encapsulates the
    `LedgerService` implementation for validator nodes. 

    The struct includes:
    1. snarkvm::Ledger, 
    2. Cache of committees organized by round
    3. Latest committee leader
    4. Shutdown flag.
    
    __No cross-dependencies__
