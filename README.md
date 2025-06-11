# Alpenglow Mini - Current Status

I threw together this mini implementation of Alpenglow after reading the whitepaper. It's mostly broken but some parts actually work.

## What Actually Works

**Blokstor**: Block storage is solid. Handles parent-child relationships, chain validation, emergency resets. The basic blockchain structure is there.

**Votor (partially)**: The voting state machine logic is correct - knows when to cast NotarVotes vs SkipVotes, handles timeouts properly. The decision-making part works.

**Network**: UDP transport is fine. Nodes talk to each other, messages serialize/deserialize correctly.

**Leader Schedule**: Basic round-robin rotation works, just not following the 4-slot window spec.

## Architectural Problems

**Leader Windows**: Currently rotating leaders every slot instead of 4-slot windows. Alpenglow explicitly requires leaders to handle 4 consecutive slots. This breaks the entire timeout and voting logic.
 
 easy fix in `get_leader(slot)` -> `(slot / 4) % validators.len()` and implement window-based timeout setting

**Certificate Creation**: Pool has `todo!()` placeholders where bls aggregation should happen. Votes get collected but certificates never get created, so consensus never actually completes.

fix: Replace with simple signature aggregation using bls multi-signatures can use first signature as placeholder for now

**Rotor Performance**: Takes 1600-2000ms(actualy shit) to disseminate blocks when slots are 400ms apart. Blocks arrive 4-5 slots late, causing everything to skip. Should be sub-100ms.

fix: skip Reed-Solomon complexity and implement the single-hop relay model, each relay should broadcasts directly.

**Missing Relay Logic**: Implemented stake-weighted relay selection but relays don't actually broadcast shreds to other nodes. Just sends to relays and stops.

fix: implement ShredReceiver relay behavior where each relay node immediately broadcasts received shreds to all other validators

## Implementation Bugs

**Reed-Solomon**: erasure coding is stubbed out, creates shreds but can't reconstruct slices properly.

**Fast vs Slow Finality**: both voting paths exist but fast-finalization (80% threshold) never triggers due to certificate creation issues.
fix: Implement concurrent voting path, run both 80% and 60% checks simultaneously

**Timeout Logic**: sets individual slot timeouts instead of window-based timeouts as specified in the whitepaper
 set timeouts for entire 4-slot windows using the formula `clock() + Δtimeout + (slot_in_window) * Δblock`

## The Real Problem

Honestly, I got the high-level architecture is good but rushed the implementation details. consensus state machines are there, the networking works, storage is fine. Just need to fix the certificate aggregation and make Rotor ACTUALLY fast.

The irony is that fixing the `todo!()` placeholders would probably get this thing working.
