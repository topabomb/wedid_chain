// SPDX-License-Identifier: Apache-2.0
// This file is part of Frontier.
//
// Copyright (c) 2022 Parity Technologies (UK) Ltd.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # EVM chain ID pallet
//!
//! The pallet that stores the numeric Ethereum-style chain id in the runtime.
//! It can simplify setting up multiple networks with different chain ID by configuring the
//! chain spec without requiring changes to the runtime config.
//!
//! **NOTE**: we recommend that the production chains still use the const parameter type, as
//! this extra storage access would imply some performance penalty.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]


pub use pallet::*;
use sp_std::{collections::btree_set::BTreeSet, iter::FromIterator, prelude::*};
use sp_core::{H160, H256, U256};

//use evm::{ExitError, ExitReason};
use fp_evm::{CallInfo, CreateInfo,ExitReason,ExitError};
use pallet_evm::{runner::Runner as RunnerT, runner::RunnerError,EvmConfig};
use hex::{FromHex,ToHex};

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event> + IsType<<Self as frame_system::Config>::Event>;
		type Banned: Get<H160>;//DApp封禁合约
	}
/*
	#[pallet::storage]
	#[pallet::getter(fn contracts)]
	pub type Contracts<T> = StorageMap<_, Blake2_128Concat, H160, u8, ValueQuery>;
	 */

	#[pallet::type_value]
	pub fn DefaultBanned<T: Config>() -> H160 {
		T::Banned::get()
	}
	#[pallet::storage]
	#[pallet::getter(fn banned)]
	pub type Banned<T> = StorageValue<_, H160, ValueQuery, DefaultBanned<T>>;

	#[pallet::genesis_config]
	pub struct GenesisConfig {
		pub banned: H160,
	}
	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig {
		fn build(&self) {
			<Banned<T>>::put(self.banned);
		}
	}
	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			Self {
				banned: H160::default(),
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event {
		BannedChanged { target: H160 },
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(10_000 + T::DbWeight::get().writes(1))]
		pub fn set_banned(origin: OriginFor<T>, target: H160) -> DispatchResult {
			ensure_root(origin)?;
			let _ = Self::set_banned_inner(target);
			Self::deposit_event(Event::BannedChanged { target });
			Ok(())
		}
	}
	pub struct AuditRunner<T: pallet_evm::Config+Config>(PhantomData<T>);
	impl<T: pallet_evm::Config+Config> RunnerT<T> for AuditRunner<T>
	where
		pallet_evm::BalanceOf<T>: TryFrom<U256> + Into<U256>,
	{
		type Error = pallet_evm::Error<T>;
		fn validate(
			source: H160,
			target: Option<H160>,
			input: Vec<u8>,
			value: U256,
			gas_limit: u64,
			max_fee_per_gas: Option<U256>,
			max_priority_fee_per_gas: Option<U256>,
			nonce: Option<U256>,
			access_list: Vec<(H160, Vec<H256>)>,
			is_transactional: bool,
			evm_config: &EvmConfig,
		) -> Result<(), RunnerError<Self::Error>> {
			return <pallet_evm::runner::stack::Runner<T>>::validate(
				source,
				target,
				input,
				value,
				gas_limit,
				max_fee_per_gas,
				max_priority_fee_per_gas,
				nonce,
				access_list,
				is_transactional,
				evm_config,
			);
		}
		fn call(
			source: H160,
			target: H160,
			input: Vec<u8>,
			value: U256,
			gas_limit: u64,
			max_fee_per_gas: Option<U256>,
			max_priority_fee_per_gas: Option<U256>,
			nonce: Option<U256>,
			access_list: Vec<(H160, Vec<H256>)>,
			is_transactional: bool,
			validate: bool,
			config: &EvmConfig,
		) -> Result<CallInfo, RunnerError<Self::Error>> {
			if is_transactional{
				let source_empty=pallet_evm::Pallet::<T>::is_account_empty(&source);
				let target_empty=pallet_evm::Pallet::<T>::is_account_empty(&target);
				if !target_empty {
					let banned=pallet::Pallet::<T>::get_banned_inner();
					if target!=banned{
						let target_bytes=target.as_fixed_bytes();
						//97f735d5000000000000000000000000=function isBanned(address dapp) public view returns (bool)
						let func_bytes = <[u8; 16]>::from_hex("97f735d5000000000000000000000000").expect("Hex decoding failed");
						let banned_input:Vec<u8>=[func_bytes.to_vec(),target_bytes.to_vec()].concat();

						let info=match <pallet_evm::runner::stack::Runner<T>>::call(
							H160::default(),
							banned,
							banned_input.clone(),
							U256::default(),
							75000000,//view函数调用是固定值
							Option::None,
							Option::None,
							Option::None,
							Vec::with_capacity(0),
							false,
							true,
							config,
						){
							Ok(info) => info,
							Err(e) => {
								return Err(e);
							}
						};
						let resp=info.value;
						if resp.len()>0&&resp[resp.len()-1]>0 {
							log::warn!("❌ Banned it! result.value({:?}),banned({}),source:{} target:{},is_transactional({})",resp,banned,source,target,is_transactional);
							return Ok(CallInfo{
								exit_reason:ExitReason::Error(ExitError::Other("Banned.".into())),
								used_gas:U256::default(),
								value:Vec::new(),
								logs:Vec::new(),
							})
						}
						log::debug!("✅ Evm call: banned({}),source:{} target:{},is_transactional({}),gas_limit({}),max_fee_per_gas({:?}),max_priority_fee_per_gas({:?}),nonce({:?})",banned,source,target,is_transactional,gas_limit,max_fee_per_gas,max_priority_fee_per_gas,nonce);
					}
				}
			}
			return <pallet_evm::runner::stack::Runner<T>>::call(
				source,
				target,
				input,
				value,
				gas_limit,
				max_fee_per_gas,
				max_priority_fee_per_gas,
				nonce,
				access_list,
				is_transactional,
				validate,
				config,
			);
		}
		fn create(
			source: H160,
			init: Vec<u8>,
			value: U256,
			gas_limit: u64,
			max_fee_per_gas: Option<U256>,
			max_priority_fee_per_gas: Option<U256>,
			nonce: Option<U256>,
			access_list: Vec<(H160, Vec<H256>)>,
			is_transactional: bool,
			validate: bool,
			config: &EvmConfig,
		) -> Result<CreateInfo, RunnerError<Self::Error>> {
			return <pallet_evm::runner::stack::Runner<T>>::create(
				source,
				init,
				value,
				gas_limit,
				max_fee_per_gas,
				max_priority_fee_per_gas,
				nonce,
				access_list,
				is_transactional,
				validate,
				config,
			);
		}
		fn create2(
			source: H160,
			init: Vec<u8>,
			salt: H256,
			value: U256,
			gas_limit: u64,
			max_fee_per_gas: Option<U256>,
			max_priority_fee_per_gas: Option<U256>,
			nonce: Option<U256>,
			access_list: Vec<(H160, Vec<H256>)>,
			is_transactional: bool,
			validate: bool,
			config: &EvmConfig,
		) -> Result<CreateInfo, RunnerError<Self::Error>> {
			return <pallet_evm::runner::stack::Runner<T>>::create2(
				source,
				init,
				salt,
				value,
				gas_limit,
				max_fee_per_gas,
				max_priority_fee_per_gas,
				nonce,
				access_list,
				is_transactional,
				validate,
				config,
			);
		}
	}
	impl<T: Config> Pallet<T> {
		pub fn set_banned_inner(value: H160) -> Weight {
			<Banned<T>>::put(value);
			T::DbWeight::get().write
		}
		pub fn get_banned_inner() -> H160{
			<Banned<T>>::get()
		}
	}
}
