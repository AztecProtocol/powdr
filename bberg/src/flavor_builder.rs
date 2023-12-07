use crate::{
    file_writer::BBFiles,
    utils::{get_relations_imports, map_with_newline},
};

pub trait FlavorBuilder {
    #[allow(clippy::too_many_arguments)]
    fn create_flavor_hpp(
        &mut self,
        name: &str,
        relation_file_names: &[String],
        fixed: &[String],
        witness: &[String],
        all_cols: &[String],
        to_be_shifted: &[String],
        shifted: &[String],
        all_cols_and_shifts: &[String],
    );
}

/// Build the boilerplate for the flavor file
impl FlavorBuilder for BBFiles {
    fn create_flavor_hpp(
        &mut self,
        name: &str,
        relation_file_names: &[String],
        fixed: &[String],
        witness: &[String],
        all_cols: &[String],
        to_be_shifted: &[String],
        shifted: &[String],
        all_cols_and_shifts: &[String],
    ) {
        let first_poly = &witness[0];
        let includes = flavor_includes(name, relation_file_names);
        let num_precomputed = fixed.len();
        let num_witness = witness.len();
        let num_all = all_cols_and_shifts.len();

        // Top of file boilerplate
        let class_aliases = create_class_aliases();
        let relation_definitions = create_relation_definitions(name, relation_file_names);
        let container_size_definitions =
            container_size_definitions(num_precomputed, num_witness, num_all);

        // Entities classes
        let precomputed_entities = create_precomputed_entities(fixed);
        let witness_entities = create_witness_entities(witness);
        let all_entities =
            create_all_entities(all_cols, to_be_shifted, shifted, all_cols_and_shifts);

        let proving_and_verification_key = create_proving_and_verification_key();
        let polynomial_views = create_polynomial_views(first_poly);

        let commitment_labels_class = create_commitment_labels(all_cols);

        let verification_commitments = create_verifier_commitments(fixed);

        let transcript = generate_transcript(witness);

        let flavor_hpp = format!(
            "
{includes}

namespace proof_system::honk {{
namespace flavor {{

class {name}Flavor {{
    public: 
        {class_aliases}

        {container_size_definitions}

        {relation_definitions}

        static constexpr bool has_zero_row = true;

    private:
        {precomputed_entities} 

        {witness_entities}

        {all_entities}

    
        {proving_and_verification_key}


        {polynomial_views}

    {commitment_labels_class}

    {verification_commitments}

    {transcript}
}};

}} // namespace proof_system::honk::flavor
}} // namespace proof_system::honk
    
    
    "
        );

        self.write_file(&self.flavor, &format!("{}_flavor.hpp", name), &flavor_hpp);
    }
}

/// Imports located at the top of the flavor files
fn flavor_includes(name: &str, relation_file_names: &[String]) -> String {
    let relation_imports = get_relations_imports(name, relation_file_names);

    format!(
        "
#pragma once
#include \"../relation_definitions_fwd.hpp\"
#include \"barretenberg/ecc/curves/bn254/g1.hpp\"
#include \"barretenberg/commitment_schemes/kzg/kzg.hpp\"
#include \"barretenberg/polynomials/barycentric.hpp\"
#include \"barretenberg/polynomials/univariate.hpp\"

#include \"barretenberg/flavor/flavor_macros.hpp\"
#include \"barretenberg/transcript/transcript.hpp\"
#include \"barretenberg/polynomials/evaluation_domain.hpp\"
#include \"barretenberg/polynomials/polynomial.hpp\"
#include \"barretenberg/flavor/flavor.hpp\"
{relation_imports}
"
    )
}

/// Creates comma separated relations tuple file
fn create_relations_tuple(master_name: &str, relation_file_names: &[String]) -> String {
    relation_file_names
        .iter()
        .map(|name| format!("{master_name}_vm::{name}<FF>"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Create Class Aliases
///
/// Contains boilerplate defining key characteristics of the flavor class
fn create_class_aliases() -> &'static str {
    r#"
        using Curve = curve::BN254;
        using G1 = Curve::Group;
        using PCS = pcs::kzg::KZG<Curve>;

        using FF = G1::subgroup_field;
        using Polynomial = barretenberg::Polynomial<FF>;
        using PolynomialHandle = std::span<FF>;
        using GroupElement = G1::element;
        using Commitment = G1::affine_element;
        using CommitmentHandle = G1::affine_element;
        using CommitmentKey = pcs::CommitmentKey<Curve>;
        using VerifierCommitmentKey = pcs::VerifierCommitmentKey<Curve>;
    "#
}

/// Create relation definitions
///
/// Contains all of the boilerplate code required to generate relation definitions.
/// We instantiate the Relations container, which contains a tuple of all of the separate relation file
/// definitions.
///
/// We then also define some constants, making use of the preprocessor.
fn create_relation_definitions(name: &str, relation_file_names: &[String]) -> String {
    // Relations tuple = ns::relation_name_0, ns::relation_name_1, ... ns::relation_name_n (comma speratated)
    let comma_sep_relations = create_relations_tuple(name, relation_file_names);

    format!("
        using Relations = std::tuple<{comma_sep_relations}>;

        static constexpr size_t MAX_PARTIAL_RELATION_LENGTH = compute_max_partial_relation_length<Relations>();

        // BATCHED_RELATION_PARTIAL_LENGTH = algebraic degree of sumcheck relation *after* multiplying by the `pow_zeta`
        // random polynomial e.g. For \\sum(x) [A(x) * B(x) + C(x)] * PowZeta(X), relation length = 2 and random relation
        // length = 3
        static constexpr size_t BATCHED_RELATION_PARTIAL_LENGTH = MAX_PARTIAL_RELATION_LENGTH + 1;
        static constexpr size_t NUM_RELATIONS = std::tuple_size<Relations>::value;

        template <size_t NUM_INSTANCES>
        using ProtogalaxyTupleOfTuplesOfUnivariates =
            decltype(create_protogalaxy_tuple_of_tuples_of_univariates<Relations, NUM_INSTANCES>());
        using SumcheckTupleOfTuplesOfUnivariates = decltype(create_sumcheck_tuple_of_tuples_of_univariates<Relations>());
        using TupleOfArraysOfValues = decltype(create_tuple_of_arrays_of_values<Relations>());
        ")
}

/// Create the number of columns boilerplate for the flavor file
fn container_size_definitions(
    num_precomputed: usize,
    num_witness: usize,
    num_all: usize,
) -> String {
    format!("
        static constexpr size_t NUM_PRECOMPUTED_ENTITIES = {num_precomputed}; 
        static constexpr size_t NUM_WITNESS_ENTITIES = {num_witness};
        static constexpr size_t NUM_WIRES = NUM_WITNESS_ENTITIES + NUM_PRECOMPUTED_ENTITIES;
        // We have two copies of the witness entities, so we subtract the number of fixed ones (they have no shift), one for the unshifted and one for the shifted
        static constexpr size_t NUM_ALL_ENTITIES = {num_all};

    ")
}

/// Returns a Ref Vector with the given name,
///
/// The vector returned will reference the columns names given
/// Used in all entities declarations
fn return_ref_vector(name: &str, columns: &[String]) -> String {
    let comma_sep = create_comma_separated(columns);

    format!("RefVector<DataType> {name}() {{ return {{ {comma_sep} }}; }};")
}

/// list -> "list[0], list[1], ... list[n-1]"
fn create_comma_separated(list: &[String]) -> String {
    list.join(", ")
}

/// Create Precomputed Entities
///
/// Precomputed first contains a pointer view defining all of the precomputed columns
/// As-well as any polys conforming to tables / ids / permutations
fn create_precomputed_entities(fixed: &[String]) -> String {
    let pointer_view = create_flavor_members(fixed);

    let selectors = return_ref_vector("get_selectors", fixed);
    let sigma_polys = return_ref_vector("get_sigma_polynomials", &[]);
    let id_polys = return_ref_vector("get_id_polynomials", &[]);
    let table_polys = return_ref_vector("get_table_polynomials", &[]);

    format!(
        "
        template<typename DataType_>
        class PrecomputedEntities : public PrecomputedEntitiesBase {{
            public:
              using DataType = DataType_;

              {pointer_view}

              {selectors}
              {sigma_polys}
              {id_polys}
              {table_polys}
          }};
        "
    )
}

fn create_witness_entities(witness: &[String]) -> String {
    let pointer_view = create_flavor_members(witness);

    let wires = return_ref_vector("get_wires", witness);
    let sorted_polys = return_ref_vector("get_sorted_polynomials", &[]);

    format!(
        "
        template <typename DataType>
        class WitnessEntities {{
            public:

            {pointer_view}

            {wires} 
            {sorted_polys} 
        }};
        "
    )
}

/// Creates container of all witness entities and shifts
fn create_all_entities(
    all_cols: &[String],
    to_be_shifted: &[String],
    shifted: &[String],
    all_cols_and_shifts: &[String],
) -> String {
    let all_entities_flavor_members = create_flavor_members(all_cols_and_shifts);

    let wires = return_ref_vector("get_wires", all_cols_and_shifts);
    let get_unshifted = return_ref_vector("get_unshifted", all_cols);
    let get_to_be_shifted = return_ref_vector("get_to_be_shifted", to_be_shifted);
    let get_shifted = return_ref_vector("get_shifted", shifted);

    format!(
        "
        template <typename DataType>
        class AllEntities {{
            public:

            {all_entities_flavor_members}


            {wires}
            {get_unshifted}
            {get_to_be_shifted}
            {get_shifted}
        }};
        "
    )
}

fn create_proving_and_verification_key() -> &'static str {
    r#"
        public:
        class ProvingKey : public ProvingKey_<PrecomputedEntities<Polynomial>, WitnessEntities<Polynomial>> {
            public:
            // Expose constructors on the base class
            using Base = ProvingKey_<PrecomputedEntities<Polynomial>, WitnessEntities<Polynomial>>;
            using Base::Base;

            // The plookup wires that store plookup read data.
            std::array<PolynomialHandle, 0> get_table_column_wires() { return {}; };
        };

        using VerificationKey = VerificationKey_<PrecomputedEntities<Commitment>>;
    "#
}

fn create_polynomial_views(first_poly: &String) -> String {
    format!("
    using ProverPolynomials = AllEntities<PolynomialHandle>;

    using FoldedPolynomials = AllEntities<std::vector<FF>>;

    class AllValues : public AllEntities<FF> {{
        public:
          using Base = AllEntities<FF>;
          using Base::Base;
      }};
  
    class AllPolynomials : public AllEntities<Polynomial> {{
      public:
        [[nodiscard]] size_t get_polynomial_size() const {{ return this->{first_poly}.size(); }}
        [[nodiscard]] AllValues get_row(const size_t row_idx) const
        {{
            AllValues result;
            for (auto [result_field, polynomial] : zip_view(result.get_all(), get_all())) {{
                result_field = polynomial[row_idx];
            }}
            return result;
        }}
    }};


    using RowPolynomials = AllEntities<FF>;

    class PartiallyEvaluatedMultivariates : public AllEntities<Polynomial> {{
      public:
        PartiallyEvaluatedMultivariates() = default;
        PartiallyEvaluatedMultivariates(const size_t circuit_size)
        {{
            // Storage is only needed after the first partial evaluation, hence polynomials of size (n / 2)
            for (auto& poly : get_all()) {{
                poly = Polynomial(circuit_size / 2);
            }}
        }}
    }};

    /**
     * @brief A container for univariates used during Protogalaxy folding and sumcheck.
     * @details During folding and sumcheck, the prover evaluates the relations on these univariates.
     */
    template <size_t LENGTH>
    using ProverUnivariates = AllEntities<barretenberg::Univariate<FF, LENGTH>>;

    /**
     * @brief A container for univariates produced during the hot loop in sumcheck.
     */
    using ExtendedEdges = ProverUnivariates<MAX_PARTIAL_RELATION_LENGTH>;

    ")
}

fn create_flavor_members(entities: &[String]) -> String {
    let pointer_list = create_comma_separated(entities);

    format!(
        "DEFINE_FLAVOR_MEMBERS(DataType, {pointer_list})",
        pointer_list = pointer_list
    )
}

fn create_labels(all_ents: &[String]) -> String {
    let mut labels = String::new();
    for name in all_ents {
        labels.push_str(&format!(
            "Base::{name} = \"{}\"; 
            ",
            name.to_uppercase()
        ));
    }
    labels
}

fn create_commitment_labels(all_ents: &[String]) -> String {
    let labels = create_labels(all_ents);

    format!(
        "
        class CommitmentLabels: public AllEntities<std::string> {{
            private:
                using Base = AllEntities<std::string>;


            public:
                CommitmentLabels() : AllEntities<std::string>()
            {{
                {labels}
            }};
        }};
        "
    )
}

fn create_key_dereference(fixed: &[String]) -> String {
    let deref_transformation = |name: &String| format!("{name} = verification_key->{name};");

    map_with_newline(fixed, deref_transformation)
}

fn create_verifier_commitments(fixed: &[String]) -> String {
    let key_dereference = create_key_dereference(fixed);

    format!(
        "
    class VerifierCommitments : public AllEntities<Commitment> {{
      private:
        using Base = AllEntities<Commitment>;

      public:
        VerifierCommitments(const std::shared_ptr<VerificationKey>& verification_key)
        {{
            {key_dereference}
        }}
    }};
"
    )
}

fn generate_transcript(witness: &[String]) -> String {
    // Transformations
    let declaration_transform = |c: &_| format!("Commitment {c};");
    let deserialize_transform = |name: &_| {
        format!(
            "{name} = deserialize_from_buffer<Commitment>(Transcript::proof_data, num_bytes_read);",
        )
    };
    let serialize_transform =
        |name: &_| format!("serialize_to_buffer<Commitment>({name}, Transcript::proof_data);");

    // Perform Transformations
    let declarations = map_with_newline(witness, declaration_transform);
    let deserialize_wires = map_with_newline(witness, deserialize_transform);
    let serialize_wires = map_with_newline(witness, serialize_transform);

    format!("
    class Transcript : public BaseTranscript {{
      public:
        uint32_t circuit_size;

        {declarations}

        std::vector<barretenberg::Univariate<FF, BATCHED_RELATION_PARTIAL_LENGTH>> sumcheck_univariates;
        std::array<FF, NUM_ALL_ENTITIES> sumcheck_evaluations;
        std::vector<Commitment> zm_cq_comms;
        Commitment zm_cq_comm;
        Commitment zm_pi_comm;

        Transcript() = default;

        Transcript(const std::vector<uint8_t>& proof)
            : BaseTranscript(proof)
        {{}}

        void deserialize_full_transcript()
        {{
            size_t num_bytes_read = 0;
            circuit_size = deserialize_from_buffer<uint32_t>(proof_data, num_bytes_read);
            size_t log_n = numeric::get_msb(circuit_size);

            {deserialize_wires}

            for (size_t i = 0; i < log_n; ++i) {{
                sumcheck_univariates.emplace_back(
                    deserialize_from_buffer<barretenberg::Univariate<FF, BATCHED_RELATION_PARTIAL_LENGTH>>(
                        Transcript::proof_data, num_bytes_read));
            }}
            sumcheck_evaluations = deserialize_from_buffer<std::array<FF, NUM_ALL_ENTITIES>>(
                Transcript::proof_data, num_bytes_read);
            for (size_t i = 0; i < log_n; ++i) {{
                zm_cq_comms.push_back(deserialize_from_buffer<Commitment>(proof_data, num_bytes_read));
            }}
            zm_cq_comm = deserialize_from_buffer<Commitment>(proof_data, num_bytes_read);
            zm_pi_comm = deserialize_from_buffer<Commitment>(proof_data, num_bytes_read);
        }}

        void serialize_full_transcript()
        {{
            size_t old_proof_length = proof_data.size();
            Transcript::proof_data.clear();
            size_t log_n = numeric::get_msb(circuit_size);

            serialize_to_buffer(circuit_size, Transcript::proof_data);

            {serialize_wires}

            for (size_t i = 0; i < log_n; ++i) {{
                serialize_to_buffer(sumcheck_univariates[i], Transcript::proof_data);
            }}
            serialize_to_buffer(sumcheck_evaluations, Transcript::proof_data);
            for (size_t i = 0; i < log_n; ++i) {{
                serialize_to_buffer(zm_cq_comms[i], proof_data);
            }}
            serialize_to_buffer(zm_cq_comm, proof_data);
            serialize_to_buffer(zm_pi_comm, proof_data);

            // sanity check to make sure we generate the same length of proof as before.
            ASSERT(proof_data.size() == old_proof_length);
        }}
    }};
    ")
}
