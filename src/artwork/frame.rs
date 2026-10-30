//! Named client frame styles and their primary artwork variants.

use super::{
    color::ColorFlags,
    properties::{integer, rules_empty, text_empty},
};
use crate::ResolvedObject;

/// Define each client frame style and its checked integer conversion together.
macro_rules! frame_styles {
    ($($name:ident = $value:literal,)*) => {
        /// The client's `CardFrameStyle` values, including computed frame styles.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(i32)]
        pub(super) enum FrameStyle {
            $(
                #[doc = concat!("The client's `", stringify!($name), "` frame style.")]
                $name = $value,
            )*
        }

        impl FrameStyle {
            /// Convert a raw property value, treating undefined styles as invalid.
            pub(super) fn from_raw(value: i32) -> Self {
                match value {
                    // Match only defined client values; reserved gaps stay invalid.
                    $($value => Self::$name,)*
                    _ => Self::Invalid,
                }
            }
        }
    };
}

frame_styles! {
    Invalid = 0,
    Classic = 1,
    ClassicDualLand = 2,
    Modern = 3,
    ModernFlip = 4,
    ModernTextless = 5,
    ModernFullArt = 6,
    FutureSight = 7,
    FutureSightTextless = 8,
    PlanarChaos = 9,
    Planeswalker = 10,
    VanguardAvatar = 11,
    ModernLeveler = 12,
    ModernEldrazi = 13,
    ClassicToken = 14,
    ModernTokenNoText = 15,
    ModernTokenWithText = 16,
    ModernTokenEldrazi = 17,
    DoubleFacedFrontSide = 18,
    DoubleFacedBackSide = 19,
    DoubleFacedPlaneswalkerFrontSide = 20,
    DoubleFacedPlaneswalkerBackSide = 21,
    Miracle = 22,
    PlaneOrPhenomenon = 23,
    CreatureLand = 24,
    GameDay = 25,
    FuseLeftSide = 26,
    FuseRightSide = 27,
    Nyx = 28,
    NyxToken = 29,
    NyxFullArt = 30,
    PostM15 = 31,
    PostM15Token = 32,
    PostM15TokenNoText = 33,
    PostM15Planeswalker3 = 34,
    PostM15Planeswalker4 = 35,
    PostM15FullArt = 36,
    PostM15Miracle = 37,
    PostM15Eldrazi = 39,
    PostM15TokenEldrazi = 40,
    PostM15TokenEldraziWithoutText = 41,
    PostM15DoubleFacedFrontSide = 42,
    PostM15DoubleFacedBackSide = 43,
    PostM15DoubleFacedPlaneswalkerFrontSide = 44,
    OriginsPlaneswalker = 45,
    Devoid = 46,
    ZendikarExpedition = 47,
    PostM15ZendikarLand = 48,
    PostM15DoubleFacedPlaneswalker2FrontSide = 49,
    PostM15DoubleFacedPlaneswalker3BackSide = 50,
    EldraziBackSide = 51,
    Vehicle = 52,
    KaladeshInventions = 53,
    AmonkhetInvocations = 54,
    IxalanLandDoubleFaceBack = 55,
    DominariaSaga = 56,
    UnstableBasicLand = 57,
    UnhingedBasicLand = 58,
    PostM15DoubleFacedPlaneswalker4BackBlack = 59,
    RavnicaMasterpiecePlaneswalker3 = 60,
    RavnicaMasterpiecePlaneswalker4 = 61,
    UltimateMastersMasterpiece = 62,
    UltimateMastersMasterpiecePlaneswalker3 = 63,
    EldraineAdventure = 64,
    EldraineAdventureAlt = 65,
    M15CollectorBooster = 66,
    THBGodNyxAlt = 68,
    THBMinimalistBasicLand = 69,
    M20Token = 70,
    ConspiracyPost8th = 71,
    ConspiracyPostM15 = 72,
    IKOCompanion = 73,
    M15CollectorBoosterCompanion = 74,
    IKOMutate = 75,
    IKOShowcase = 76,
    IKOShowcaseToho = 77,
    IKOShowcaseTohoCompanion = 78,
    M21FireFavorites = 79,
    ZNRModalDoubleFaced = 80,
    ZNRExpeditions = 81,
    ZNRShowcase = 82,
    UnsanctionedLand = 83,
    SL2UniversesBeyondFullArt = 84,
    SL2UniversesBeyond = 85,
    SL2MetalConcertPoster = 86,
    KHMShowcase = 87,
    KHMShowcasePlaneswalker = 88,
    STXShowcase = 89,
    SL3AprilFoolsArtless = 90,
    AFRDungeon = 91,
    AFRClass = 92,
    MIDDayNightEmblem = 93,
    MIDEternalNightBasicLand = 94,
    MIDEternalNight = 95,
    DBLDoubleFeature = 96,
    DBLDoubleFeaturePlaneswalker3 = 97,
    DBLDoubleFeaturePlaneswalker4 = 98,
    SL3PixelLand = 99,
    NEOPhyrexianPlaneswalker4 = 100,
    NEOBorderlessPhyrexianPlaneswalker4 = 101,
    NEOUkiyoELand = 102,
    NEOSagaTDFC = 103,
    SLDUniversesBeyondTDFC = 104,
    SLDUniversesBeyondFullArtNyx = 105,
    STXShowcaseJP = 106,
    SNCLandFullTextureless = 107,
    CMRFoilEtched = 108,
    CLBInitiativeEmblem = 109,
    Unique2X2CrypticSpires = 110,
    BROPrototype = 111,
    BROPrototypeExtendedArt = 112,
    BROMinimalistBasicLand = 113,
    BROPlaneswalker6MeldBack = 114,
    BROMeldBack = 115,
    KHMPhyrexian = 116,
    ONEManaBarBasicLand = 117,
    ONECompleatOilBasicLand = 118,
    ONEShowcase = 119,
    ONEShowcasePlaneswalker3 = 120,
    ONEShowcasePlaneswalker4 = 121,
    SLDUniversesBeyondBorderlessAltName = 122,
    SLDUniversesBeyondSaga = 123,
    ONEFireFavoritesWithClip = 124,
    SLDUniversesBeyondFoilEtched = 125,
    MOMInnistradMIDShowcase = 126,
    MOMRavnicaShowcase = 127,
    MOMTarkirShowcase = 128,
    MOMInnistradVOWShowcase = 129,
    MOMKamigawaShowcase = 130,
    MOMEldraineShowcase = 131,
    MOMDominariaShowcase = 132,
    MOMNewCapennaShowcase = 133,
    MOMIxalanShowcase = 134,
    MOMIkoriaShowcase = 135,
    MOMBorderlessTheros = 136,
    MOMBattle = 137,
    FNMDark = 138,
    CMRFoilEtchedCompanion = 139,
    MOMAmonkhetShowcase = 140,
    LTRUniversesBeyondManaBarBasicLand = 141,
    LTRRingEmblem = 142,
    LTRUniversesBeyondExtendedArt = 143,
    LTRUniversesBeyondBorderlessScene = 144,
    LTRShowcaseRing = 145,
    LTRUniversesBeyondFireFavorite = 146,
    CMMFoilEtchedPlaneswalker4 = 147,
    CMMFoilEtchedPlaneswalker3 = 148,
    UNFShowcase = 149,
    UnfinityLand = 150,
    UNFShockLand = 151,
    WOTShowcase = 152,
    WOEFireFavoriteAdventure = 153,
    ZendikarBasicLand = 154,
    LTRFinalBattle = 155,
    LTRFinalBattleSaga = 156,
    LCIIxalanShowcaseRedux = 157,
    LCICaveLandDoubleFaceBack = 158,
    LCITravelPosterLand = 159,
    LCIMesoAmericanShowcase = 160,
    LCICosmiumNeonInk = 161,
    LCIExtendedArtAdventure = 162,
    MKMCase = 163,
    MKMDossiers = 164,
    MKMShowcaseStory = 165,
    MKMDossiersEclipse = 166,
    LTRUniversesBeyondFireFavoriteWithClip = 167,
    CLUUniversesBeyondFireFavorite = 168,
    CLUUniversesBeyondFireFavoriteWithClip = 169,
    OTPShowcaseCrimes = 170,
    OTPShowcaseCrimesPlaneswalker3 = 171,
    OTPEclipseCrimes = 172,
    OTPEclipseCrimesPlaneswalker3 = 173,
    OTJShowcaseWanted = 174,
    OTJShowcaseWantedPlaneswalker4 = 175,
    OTJSpree = 176,
    OTJExtendedArtSpree = 177,
    BIGShowcaseVault = 178,
    BIGEclipseVault = 179,
    OTJBorderlessFavorite = 180,
    PostM15DoubleFacedPlaneswalker4BackWhite = 181,
    OTJBorderlessFavoriteRippleFoil = 182,
    ACRShowcaseMemoryCorridor = 183,
    BLBBorderlessAlt = 184,
    BLBBorderlessAltPlaneswalker3 = 185,
    BLBBorderlessAltPlaneswalker4 = 186,
    BLBShowcaseWoodland = 187,
    DSKDoubleExposure = 188,
    DSKDoubleExposurePlaneswalker4 = 189,
    DSKDoubleExposureEclipse = 190,
    DSKDoubleExposureEclipsePlaneswalker4 = 191,
    DSKHauntedTech = 192,
    MinimalistShowcase = 193,
    DSKRoom = 194,
    DSKBorderlessRoom = 195,
    PIPPipBoyAltName = 196,
    PIPPipBoy = 197,
    UBAdventure = 198,
    UBExtendedArtAdventure = 199,
    TOKPlayAid = 200,
    SNCHeroDeco = 201,
    MinimalistShowcasePlaneswalker4 = 202,
    SNCHeroDecoPW3 = 203,
    SNCHeroDecoPW4 = 204,
    OmenStandard = 205,
    DFTGoldPrizePlaneswalker4 = 206,
    INRInnistradMIDShowcasePW3 = 207,
    DFTGoldPrizeManabar = 208,
    DFTGoldPrizeBorderless = 209,
    DFTGoldPrizeBorderlessNoTitlebar = 210,
    DFTGoldPrizeBase = 211,
    BorderlessNoTitlebar = 212,
    TDMDragonShowcase = 213,
    TDMDragonShowcaseOmen = 214,
    BorderlessSaga = 215,
    BorderlessOmen = 216,
    TDMUginShowcase = 217,
    TDMUginShowcasePW4 = 218,
    TDMClansShowcase = 219,
    UBCreatureSaga = 220,
    UBBorderlessCreatureSaga = 221,
    UBBorderlessAdventure = 222,
    FINCanvas = 223,
    UBMeldBack = 224,
    UBMeldBackWithClip = 225,
    FINGoldChocobo = 226,
    UniversesBeyondBorderlessGeneric = 227,
    EOSShadowLand = 228,
    EOSStarFieldLand = 229,
    StationStandard = 230,
    BorderlessManaBarNoTitlebar = 231,
    EOEPortalLand = 232,
    EOEStationPortalLand = 233,
    ExtendedArtStation = 234,
    BorderlessStation = 235,
    TMDFCStandard = 236,
    MinimalistNeonShowcase = 237,
    TLAElementalShowcase = 238,
    BorderlessScene = 239,
    CanvasOutlineText = 240,
    TLABorderlessHeadliner = 241,
    ECLBedtimeStoryShowcase = 242,
    StandardAltName = 243,
    TMTSewerShowcase = 244,
    TMTBorderlessPixel = 245,
    SOSMysticalArchive = 246,
    StandardPrepared = 247,
    ExtendedArtPrepared = 248,
    ExtendedArtAltName = 249,
    BorderlessOutlineTitle = 250,
    MSHComicPanelShowcase = 251,
    MSHComicPanelShowcaseSaga = 252,
    SPMWebShowcase = 253,
    BorderlessSceneAdventure = 254,
    HOBDragonHoardShowcaseStandard = 255,
    HOBDragonHoardShowcaseAdventure = 256,
    HOBBookCoverStandard = 257,
    HOBBookCoverAdventure = 258,
    FRATowerShowcase = 259,
    TokenPlaneswalker = 260,
    BorderlessSceneManaBar = 261,
    FuseSplitCard = 262,
    SplitCard = 263,
    AbilityOrEffect = 264,
    FaceDown = 265,
    Morph = 266,
    Manifest = 267,
    TezzeretCreature = 268,
    YedoraLand = 269,
    Foretold = 270,
    Emblem = 271,
    DominariaSagaAbility = 272,
    AFRClassAbility = 273,
    Cloak = 274,
}

impl FrameStyle {
    /// Whether the client routes a style through its token-specific art rules.
    pub(super) fn is_token(self) -> bool {
        use FrameStyle::*;

        matches!(
            self,
            ClassicToken
                | ModernTokenNoText
                | ModernTokenWithText
                | ModernTokenEldrazi
                | NyxToken
                | PostM15Token
                | PostM15TokenNoText
                | PostM15TokenEldrazi
                | PostM15TokenEldraziWithoutText
                | TokenPlaneswalker
        )
    }

    /// Whether the client uses planeswalker-specific primary images for this style.
    pub(super) fn is_planeswalker(self) -> bool {
        use FrameStyle::*;

        matches!(
            self,
            Planeswalker
                | DoubleFacedPlaneswalkerFrontSide
                | DoubleFacedPlaneswalkerBackSide
                | PostM15Planeswalker3
                | PostM15Planeswalker4
                | PostM15DoubleFacedPlaneswalkerFrontSide
                | OriginsPlaneswalker
                | PostM15DoubleFacedPlaneswalker2FrontSide
                | PostM15DoubleFacedPlaneswalker3BackSide
                | PostM15DoubleFacedPlaneswalker4BackBlack
                | RavnicaMasterpiecePlaneswalker3
                | RavnicaMasterpiecePlaneswalker4
                | UltimateMastersMasterpiecePlaneswalker3
                | KHMShowcasePlaneswalker
                | DBLDoubleFeaturePlaneswalker3
                | DBLDoubleFeaturePlaneswalker4
                | NEOPhyrexianPlaneswalker4
                | NEOBorderlessPhyrexianPlaneswalker4
                | BROPlaneswalker6MeldBack
                | ONEShowcasePlaneswalker3
                | ONEShowcasePlaneswalker4
                | CMMFoilEtchedPlaneswalker4
                | CMMFoilEtchedPlaneswalker3
                | OTPShowcaseCrimesPlaneswalker3
                | OTPEclipseCrimesPlaneswalker3
                | OTJShowcaseWantedPlaneswalker4
                | PostM15DoubleFacedPlaneswalker4BackWhite
                | BLBBorderlessAltPlaneswalker3
                | BLBBorderlessAltPlaneswalker4
                | DSKDoubleExposurePlaneswalker4
                | DSKDoubleExposureEclipsePlaneswalker4
                | MinimalistShowcasePlaneswalker4
                | SNCHeroDecoPW3
                | SNCHeroDecoPW4
                | DFTGoldPrizePlaneswalker4
                | INRInnistradMIDShowcasePW3
                | TDMUginShowcasePW4
                | TokenPlaneswalker
        )
    }

    /// Select the token crop from style, color, and text-box presence.
    pub(super) fn token_suffix(
        self,
        object: &ResolvedObject<'_>,
        color: ColorFlags,
    ) -> Option<&'static str> {
        use FrameStyle::*;

        let flavor_empty = text_empty(object, "FLAVORTEXT_STRING")?;
        let rules_empty = rules_empty(object)?;
        Some(match self {
            TokenPlaneswalker => "_typ_planeswk_token",
            ModernTokenEldrazi if color == ColorFlags::COLORLESS => "_typ_tok_sty_020",
            PostM15TokenEldrazi => "_typ_tok_sty_020",
            ClassicToken
                if FrameStyle::from_raw(integer(object, "FRAMESTYLE", Invalid as i32)?)
                    == ClassicToken =>
            {
                // Classic tokens distinguish rules and flavor independently.
                match (rules_empty, flavor_empty) {
                    (true, true) => "_typ_tokntb_sty_012",
                    (false, true) => "_typ_tokwtb_sty_011",
                    (_, false) => "_typ_tokwtb_sty_002",
                }
            }
            ClassicToken | ModernTokenNoText | PostM15TokenEldraziWithoutText => {
                "_typ_tokntb_sty_012"
            }
            ModernTokenWithText => "_typ_tokwtb_sty_011",
            _ if rules_empty && flavor_empty => "_typ_tokntb_sty_012",
            _ => "_typ_tokwtb_sty_011",
        })
    }

    /// Select the primary planeswalker image, including special showcase treatments.
    pub(super) fn planeswalker_suffix(self) -> &'static str {
        use FrameStyle::*;

        match self {
            CMMFoilEtchedPlaneswalker4 | CMMFoilEtchedPlaneswalker3 => "_typ_planeswk_sty_010",
            OTPShowcaseCrimesPlaneswalker3 => "_typ_planeswk_otp_sty_040",
            OTPEclipseCrimesPlaneswalker3 => "_typ_planeswk_otp_sty_050",
            OTJShowcaseWantedPlaneswalker4 => "_typ_planeswk_otj_sty_040",
            DSKDoubleExposureEclipsePlaneswalker4 => "_typ_dsk_dee_sty_010",
            MinimalistShowcasePlaneswalker4 => "_typ_min_planeswk_sty_050",
            SNCHeroDecoPW3 | SNCHeroDecoPW4 => "_typ_snc_deco_planeswk_sty_010",
            INRInnistradMIDShowcasePW3 => "_type_inr_hrvst_planeswk_sty_050",
            DFTGoldPrizePlaneswalker4 => "_typ_planeswk_gold_sty_035",
            TDMUginShowcasePW4 => "_typ_planeswk_ugin_sty_035",
            _ => "_typ_planeswk_sty_040",
        }
    }
}

#[cfg(test)]
mod tests {
    //! Boundary cases in the client's integer frame-style representation.

    use super::FrameStyle;

    /// Defined avatar values and reserved gaps must retain distinct meanings.
    #[test]
    fn raw_frame_styles_check_reserved_values() {
        assert_eq!(FrameStyle::from_raw(11), FrameStyle::VanguardAvatar);
        for value in [-1, 38, 67, 275] {
            // The client rejects negative, reserved, and out-of-range enum values.
            assert_eq!(FrameStyle::from_raw(value), FrameStyle::Invalid);
        }
    }
}
