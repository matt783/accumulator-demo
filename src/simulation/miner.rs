use super::state::{Block, Transaction};
use super::util;
use accumulator::group::UnknownOrderGroup;
use accumulator::Accumulator;
use multiqueue::{BroadcastReceiver, BroadcastSender};
use rug::Integer;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

pub struct Miner<G: UnknownOrderGroup> {
  acc: Accumulator<G>,
  block_height: u64,
  pending_transactions: Vec<Transaction<G>>,
}

impl<G: UnknownOrderGroup> Miner<G> {
  /// Assumes all miners are online from genesis. We may want to implement syncing later.
  pub fn start(
    is_leader: bool,
    acc: Accumulator<G>,
    block_interval_ms: u64,
    block_sender: BroadcastSender<Block<G>>,
    block_receiver: BroadcastReceiver<Block<G>>,
    tx_receiver: BroadcastReceiver<Transaction<G>>,
  ) {
    let miner_ref = Arc::new(Mutex::new(Miner {
      acc,
      block_height: 0,
      pending_transactions: Vec::new(),
    }));

    // Transaction processor thread.
    let miner = miner_ref.clone();
    let transaction_thread = thread::spawn(move || {
      for tx in tx_receiver {
        miner.lock().unwrap().add_transaction(tx);
      }
    });

    // Block validation thread.
    let miner = miner_ref.clone();
    let validate_thread = thread::spawn(move || {
      for block in block_receiver {
        miner.lock().unwrap().validate_block(block);
      }
    });

    // Block creation on an interval.
    if is_leader {
      loop {
        sleep(Duration::from_millis(block_interval_ms));
        let new_block = miner_ref.lock().unwrap().forge_block();
        // Note: This miner will consume the forged block via validate.
        block_sender.try_send(new_block).unwrap();
      }
    }

    transaction_thread.join().unwrap();
    validate_thread.join().unwrap();
  }

  fn add_transaction(&mut self, transaction: Transaction<G>) {
    // This contains check could incur overhead; ideally we'd use a set but Rust HashSet is kind of
    // a pain to use here.
    if !self.pending_transactions.contains(&transaction) {
      self.pending_transactions.push(transaction);
    }
  }

  fn forge_block(&self) -> Block<G> {
    let (elems_added, elems_deleted) = util::elems_from_transactions(&self.pending_transactions);
    let (witness_deleted, proof_deleted) = self.acc.clone().delete(&elems_deleted).unwrap();
    let (acc_new, proof_added) = witness_deleted.clone().add(&elems_added);
    Block {
      height: self.block_height + 1,
      transactions: self.pending_transactions.clone(),
      acc_new,
      proof_added,
      proof_deleted,
    }
  }

  fn validate_block(&mut self, block: Block<G>) {
    // Preserves idempotency if multiple miners are leaders.
    if block.height != self.block_height + 1 {
      return;
    }

    let (elems_added, elem_witnesses_deleted) = util::elems_from_transactions(&block.transactions);
    let elems_deleted: Vec<Integer> = elem_witnesses_deleted
      .iter()
      .map(|(u, _wit)| u.clone())
      .collect();
    assert!(self
      .acc
      .verify_membership(&elems_deleted, &block.proof_deleted));
    assert!(block
      .acc_new
      .verify_membership(&elems_added, &block.proof_added));
    assert!(block.proof_deleted.witness == block.proof_added.witness);
    self.acc = block.acc_new;
    self.block_height = block.height;
    self.pending_transactions.clear();
  }
}
