use crate::{file_writer::BBFiles, utils::map_with_newline};

pub trait VerifierBuilder {
    fn create_verifier_cpp(&mut self, name: &str, witness: &[String]);

    fn create_verifier_hpp(&mut self, name: &str);
}

impl VerifierBuilder for BBFiles {
    fn create_verifier_cpp(&mut self, name: &str, witness: &[String]) {
        let include_str = includes_cpp(name);

        let wire_transformation = |n: &String| {
            format!(
            "commitments.{n} = transcript->template receive_from_prover<Commitment>(commitment_labels.{n});"
        )
        };
        let wire_commitments = map_with_newline(witness, wire_transformation);

        let ver_cpp = format!("
{include_str} 

    namespace bb {{
    {name}Verifier::{name}Verifier(std::shared_ptr<Flavor::VerificationKey> verifier_key)
        : key(verifier_key)
    {{}}
    
    {name}Verifier::{name}Verifier({name}Verifier&& other) noexcept
        : key(std::move(other.key))
        , pcs_verification_key(std::move(other.pcs_verification_key))
    {{}}
    
    {name}Verifier& {name}Verifier::operator=({name}Verifier&& other) noexcept
    {{
        key = other.key;
        pcs_verification_key = (std::move(other.pcs_verification_key));
        commitments.clear();
        return *this;
    }}
    
    /**
     * @brief This function verifies an {name} Honk proof for given program settings.
     *
     */
    bool {name}Verifier::verify_proof(const HonkProof& proof)
    {{
        using Flavor = {name}Flavor;
        using FF = Flavor::FF;
        using Commitment = Flavor::Commitment;
        // using Curve = Flavor::Curve;
        // using ZeroMorph = ZeroMorphVerifier_<Curve>;
        using VerifierCommitments = Flavor::VerifierCommitments;
        using CommitmentLabels = Flavor::CommitmentLabels;
    
        RelationParameters<FF> relation_parameters;
    
        transcript = std::make_shared<Transcript>(proof);
    
        VerifierCommitments commitments {{ key }};
        CommitmentLabels commitment_labels;
    
        const auto circuit_size = transcript->template receive_from_prover<uint32_t>(\"circuit_size\");
    
        if (circuit_size != key->circuit_size) {{
            return false;
        }}
    
        // Get commitments to VM wires
        {wire_commitments}
    
        // Execute Sumcheck Verifier
        const size_t log_circuit_size = numeric::get_msb(circuit_size);
        auto sumcheck = SumcheckVerifier<Flavor>(log_circuit_size, transcript);

        FF alpha = transcript->template get_challenge<FF>(\"Sumcheck:alpha\");

        auto gate_challenges = std::vector<FF>(log_circuit_size);
        for (size_t idx = 0; idx < log_circuit_size; idx++) {{
            gate_challenges[idx] = transcript->template get_challenge<FF>(\"Sumcheck:gate_challenge_\" + std::to_string(idx));
        }}

        auto [multivariate_challenge, claimed_evaluations, sumcheck_verified] =
            sumcheck.verify(relation_parameters, alpha, gate_challenges);
    
        // If Sumcheck did not verify, return false
        if (sumcheck_verified.has_value() && !sumcheck_verified.value()) {{
            return false;
        }}
    
        // Execute ZeroMorph rounds. See https://hackmd.io/dlf9xEwhTQyE3hiGbq4FsA?view for a complete description of the
        // unrolled protocol.
        // NOTE: temporarily disabled - facing integration issues
        // auto pairing_points = ZeroMorph::verify(commitments.get_unshifted(),
        //                                         commitments.get_to_be_shifted(),
        //                                         claimed_evaluations.get_unshifted(),
        //                                         claimed_evaluations.get_shifted(),
        //                                         multivariate_challenge,
        //                                         transcript);
    
        // auto verified = pcs_verification_key->pairing_check(pairing_points[0], pairing_points[1]);
        // return sumcheck_verified.value() && verified;
        return sumcheck_verified.value();
    }}
    
    
    }} // namespace bb
    
    
    ");

        self.write_file(&self.prover, &format!("{}_verifier.cpp", name), &ver_cpp);
    }

    fn create_verifier_hpp(&mut self, name: &str) {
        let include_str = include_hpp(name);
        let ver_hpp = format!(
            "
{include_str}
    
    namespace bb {{
    class {name}Verifier {{
        using Flavor = {name}Flavor;
        using FF = Flavor::FF;
        using Commitment = Flavor::Commitment;
        using VerificationKey = Flavor::VerificationKey;
        using VerifierCommitmentKey = Flavor::VerifierCommitmentKey;
        using Transcript = Flavor::Transcript;
    
    public:
        explicit {name}Verifier(std::shared_ptr<VerificationKey> verifier_key = nullptr);
        {name}Verifier({name}Verifier&& other) noexcept;
        {name}Verifier(const {name}Verifier& other) = delete;
    
        {name}Verifier& operator=(const {name}Verifier& other) = delete;
        {name}Verifier& operator=({name}Verifier&& other) noexcept;
    
        bool verify_proof(const HonkProof& proof);
    
        std::shared_ptr<VerificationKey> key;
        std::map<std::string, Commitment> commitments;
        std::shared_ptr<VerifierCommitmentKey> pcs_verification_key;
        std::shared_ptr<Transcript> transcript;
    }};
    
    }} // namespace bb
     
    
    "
        );

        self.write_file(&self.prover, &format!("{}_verifier.hpp", name), &ver_hpp);
    }
}

fn include_hpp(name: &str) -> String {
    format!(
        "
#pragma once
#include \"barretenberg/flavor/generated/{name}_flavor.hpp\"
#include \"barretenberg/sumcheck/sumcheck.hpp\"
#include \"barretenberg/plonk/proof_system/types/proof.hpp\"
"
    )
}

fn includes_cpp(name: &str) -> String {
    format!(
        "
    #include \"./{name}_verifier.hpp\"
    #include \"barretenberg/commitment_schemes/zeromorph/zeromorph.hpp\"
    #include \"barretenberg/numeric/bitop/get_msb.hpp\"
    #include \"barretenberg/transcript/transcript.hpp\"
    "
    )
}
