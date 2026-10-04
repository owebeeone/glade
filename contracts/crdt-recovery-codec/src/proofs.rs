//! Structural proof envelopes; success does not mean signature or permission valid.
use crate::*;
pub trait ProofBody: ProfileBody {
    const SIGNATURE_DOMAIN: &'static str;
}
macro_rules! body {
    ($ty:ident,$domain:ident) => {
        impl ProofBody for $ty {
            const SIGNATURE_DOMAIN: &'static str = $domain;
        }
    };
}
body!(SignedDescriptorBody, DESCRIPTOR_DOMAIN);
body!(InventoryHeaderBody, INVENTORY_DOMAIN);
body!(CertificateBody, CERTIFICATE_DOMAIN);
body!(PolicyBody, POLICY_DOMAIN);
body!(PermitBody, PERMIT_DOMAIN);
body!(OperationProofBody, OPERATION_DOMAIN);
body!(LocalChallengeBody, CHALLENGE_DOMAIN);
body!(LocalResponseBody, RESPONSE_DOMAIN);
body!(OriginalAdmissionBody, ADMISSION_DOMAIN);
body!(ExchangeHello, HELLO_DOMAIN);
pub fn decode_signed_body<T: ProofBody>(
    record: &SignedRecord,
    limits: &DecodeLimits,
) -> Result<T, Error> {
    if record.purpose != T::SIGNATURE_DOMAIN
        || record.version != 1
        || record.signature.value().len() != 64
    {
        return Err(Error::Invalid);
    }
    decode_profile(record.body.value(), limits)
}
