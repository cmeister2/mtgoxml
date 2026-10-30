//! Primary artwork filename selection from effective MTGO card properties.

use super::{
    color::ColorFlags,
    frame::FrameStyle,
    properties::{boolean, colors, integer},
};

use crate::ResolvedObject;

// MTGO 3.4.159.4708: CardDefinition.GetArtSuffix and
// DigitalMagicObject.GetFrameStyle, WotC.MtGO.Client.Model.Core.dll SHA-256
// 0b315e11a92a96585a8d5c85cac268b2ecd6a68df6d07459139de6063f449527.
// Frame styles include the client's computed property-based overrides.

/// Select the primary image suffix, excluding avatars and client placeholders.
pub(super) fn primary_suffix(object: &ResolvedObject<'_>) -> Option<&'static str> {
    use FrameStyle::*;

    if object.field_ref("IS_DIGITALOBJECT").is_some() {
        // The client routes this marker to its separate store-image directory.
        return None;
    }
    let frame = frame_style(object)?;
    if frame == VanguardAvatar
        || boolean(object, "IS_VANGUARD")?
        || boolean(object, "MTGO_CLIENT_NON_CARD")?
    {
        return None;
    }
    let color = colors(object)?;
    if frame.is_token() {
        return frame.token_suffix(object, color);
    }
    if boolean(object, "FLIP_CARD")? {
        return Some(
            if matches!(
                color,
                ColorFlags::WHITE | ColorFlags::BLUE | ColorFlags::BLACK | ColorFlags::GREEN
            ) {
                "_typ_flip_sty_013"
            } else {
                "_typ_reg_sty_010"
            },
        );
    }
    if frame.is_planeswalker() {
        return Some(frame.planeswalker_suffix());
    }
    let colorless_spell = color == ColorFlags::COLORLESS
        && !boolean(object, "IS_ARTIFACT")?
        && !boolean(object, "IS_LAND")?;
    Some(match frame {
        SL3AprilFoolsArtless => return None,
        // Full-art, saga, land, borderless, and showcase primary images.
        ModernTextless
        | ModernFullArt
        | GameDay
        | NyxFullArt
        | DominariaSaga
        | UnstableBasicLand
        | UnhingedBasicLand
        | THBGodNyxAlt
        | THBMinimalistBasicLand
        | IKOMutate
        | IKOShowcase
        | IKOShowcaseToho
        | IKOShowcaseTohoCompanion
        | M21FireFavorites
        | ZNRExpeditions
        | ZNRShowcase
        | UnsanctionedLand
        | SL2UniversesBeyondFullArt
        | SL2MetalConcertPoster
        | STXShowcase
        | AFRClass
        | MIDDayNightEmblem
        | MIDEternalNightBasicLand
        | MIDEternalNight
        | SL3PixelLand
        | NEOUkiyoELand
        | NEOSagaTDFC
        | SLDUniversesBeyondFullArtNyx
        | STXShowcaseJP
        | SNCLandFullTextureless
        | CLBInitiativeEmblem
        | Unique2X2CrypticSpires
        | BROMinimalistBasicLand
        | ONEManaBarBasicLand
        | ONECompleatOilBasicLand
        | ONEShowcase
        | SLDUniversesBeyondBorderlessAltName
        | SLDUniversesBeyondSaga
        | ONEFireFavoritesWithClip
        | MOMInnistradMIDShowcase
        | MOMKamigawaShowcase
        | MOMNewCapennaShowcase
        | MOMIkoriaShowcase
        | MOMBorderlessTheros
        | MOMBattle
        | LTRUniversesBeyondManaBarBasicLand
        | LTRUniversesBeyondBorderlessScene
        | LTRUniversesBeyondFireFavorite
        | UnfinityLand
        | UNFShockLand
        | WOEFireFavoriteAdventure
        | ZendikarBasicLand
        | LCITravelPosterLand
        | LCIMesoAmericanShowcase
        | LTRUniversesBeyondFireFavoriteWithClip
        | CLUUniversesBeyondFireFavorite
        | CLUUniversesBeyondFireFavoriteWithClip
        | OTPEclipseCrimes
        | OTJBorderlessFavorite
        | OTJBorderlessFavoriteRippleFoil
        | BLBBorderlessAlt
        | DSKDoubleExposure
        | BorderlessNoTitlebar
        | BorderlessSaga
        | BorderlessOmen
        | UBBorderlessCreatureSaga
        | UBBorderlessAdventure
        | UniversesBeyondBorderlessGeneric
        | BorderlessManaBarNoTitlebar
        | BorderlessStation
        | MinimalistNeonShowcase
        | BorderlessScene
        | TLABorderlessHeadliner
        | TMTBorderlessPixel
        | SOSMysticalArchive
        | BorderlessOutlineTitle
        | MSHComicPanelShowcase
        | MSHComicPanelShowcaseSaga
        | SPMWebShowcase
        | BorderlessSceneAdventure
        | BorderlessSceneManaBar
        | DominariaSagaAbility
        | AFRClassAbility => "_typ_reg_sty_050",
        MKMCase if colorless_spell => "_typ_case_sty_050",
        MKMCase => "_typ_case_sty_010",
        PostM15FullArt => "_typ_old_sty_050",
        LTRFinalBattleSaga => "_typ_fb_sty_050",
        // Universes Beyond and etched treatments retain a crop for colored cards.
        ZNRModalDoubleFaced
        | SL2UniversesBeyond
        | SLDUniversesBeyondTDFC
        | CMRFoilEtched
        | SLDUniversesBeyondFoilEtched
        | UBAdventure
            if colorless_spell =>
        {
            "_typ_reg_sty_050"
        }
        ZNRModalDoubleFaced
        | SL2UniversesBeyond
        | SLDUniversesBeyondTDFC
        | CMRFoilEtched
        | SLDUniversesBeyondFoilEtched
        | UBAdventure => "_typ_reg_sty_010",
        FutureSight | ZendikarExpedition | PostM15ZendikarLand => "_typ_reg_sty_020",
        IxalanLandDoubleFaceBack
        | EldraineAdventureAlt
        | KHMShowcase
        | MOMEldraineShowcase
        | WOTShowcase
        | LCICaveLandDoubleFaceBack => "_typ_reg_sty_030",
        LTRFinalBattle => "_typ_fb_sty_010",
        // Extended-art colorless nonartifact spells have a separate crop.
        UltimateMastersMasterpiece
        | M15CollectorBooster
        | M15CollectorBoosterCompanion
        | BROPrototypeExtendedArt
        | LTRUniversesBeyondExtendedArt
        | LCIExtendedArtAdventure
        | OTJExtendedArtSpree
        | UBExtendedArtAdventure
        | ExtendedArtPrepared
        | ExtendedArtAltName
            if colorless_spell =>
        {
            "_typ_reg_sty_035"
        }
        UltimateMastersMasterpiece
        | M15CollectorBooster
        | M15CollectorBoosterCompanion
        | BROPrototypeExtendedArt
        | LTRUniversesBeyondExtendedArt
        | LCIExtendedArtAdventure
        | OTJExtendedArtSpree
        | UBExtendedArtAdventure
        | ExtendedArtPrepared
        | ExtendedArtAltName => "_typ_reg_sty_030",
        LCICosmiumNeonInk => "_typ_neon_sty_050",
        PlanarChaos | ModernLeveler | PlaneOrPhenomenon | FuseLeftSide | FuseRightSide | Nyx => {
            "_typ_reg_sty_010"
        }
        FutureSightTextless => "_typ_tok_sty_020",
        LTRRingEmblem if boolean(object, "IS_FRONT_FACE")? => "_front_typ_reg_sty_010",
        LTRRingEmblem => "_back_typ_reg_sty_010",
        MKMDossiers | MKMDossiersEclipse => "_typ_dsr_sty_010",
        MKMShowcaseStory => "_typ_mfg_sty_030",
        OTPShowcaseCrimes => "_typ_otp_sty_010",
        OTJShowcaseWanted => "_typ_otj_sty_010",
        BIGShowcaseVault | BIGEclipseVault => "_typ_big_sty_010",
        Classic => "_typ_reg_sty_001",
        ACRShowcaseMemoryCorridor => "_typ_acr_mc_sty_010",
        BLBShowcaseWoodland => "_typ_blb_wl_sty_010",
        DSKHauntedTech => "_typ_dsk_ht_sty_010",
        MinimalistShowcase => "_typ_min_sty_010",
        DSKDoubleExposureEclipse => "_typ_dsk_dee_sty_010",
        DSKRoom => "_typ_room_sty_020",
        DSKBorderlessRoom => "_typ_room_sty_050",
        PIPPipBoyAltName | PIPPipBoy => "_typ_pip_pip_sty_010",
        TOKPlayAid => "_typ_tok_aid_sty_050",
        SNCHeroDeco => "_typ_snc_deco_sty_010",
        BROMeldBack | UBMeldBack | UBMeldBackWithClip => "_typ_meld_sty_050",
        DFTGoldPrizeManabar => "_typ_reg_gold_MB_sty_050",
        DFTGoldPrizeBorderless | DFTGoldPrizeBorderlessNoTitlebar => "_typ_reg_gold_sty_050",
        DFTGoldPrizeBase => "_typ_reg_gold_sty_010",
        TDMClansShowcase => "_typ_clan_sty_050",
        TDMUginShowcase => "_typ_ugin_sty_050",
        TDMDragonShowcase | TDMDragonShowcaseOmen => "_typ_drgn_sty_030",
        UBCreatureSaga if colorless_spell => "_typ_csaga_sty_050",
        UBCreatureSaga => "_typ_csaga_sty_010",
        FINCanvas | CanvasOutlineText | HOBBookCoverStandard | HOBBookCoverAdventure => {
            "_typ_canvas_sty_050"
        }
        FINGoldChocobo => "_typ_gold_cbo_sty_050",
        ExtendedArtStation => "_typ_ex_station",
        EOEPortalLand | EOEStationPortalLand => "_typ_eoe_portal",
        StationStandard => "_typ_station",
        EOSStarFieldLand => "_typ_eos_star_field",
        EOSShadowLand => "_typ_eos_shadow",
        TLAElementalShowcase => "_typ_tla_elemental",
        ECLBedtimeStoryShowcase => "_typ_ecl_bedtime",
        TMTSewerShowcase => "_typ_tmt_sewer",
        HOBDragonHoardShowcaseStandard | HOBDragonHoardShowcaseAdventure => "_typ_hob_dragon_hoard",
        FRATowerShowcase => "_typ_fra_tower",
        ModernEldrazi
        | ModernTokenEldrazi
        | PostM15Eldrazi
        | PostM15TokenEldrazi
        | PostM15TokenEldraziWithoutText
        | Devoid
        | EldraziBackSide
        | KaladeshInventions
        | Emblem => "_typ_reg_sty_050",
        ClassicDualLand if object.set().id != "Sort" => "_typ_reg_sty_001",
        // The client's 8th-edition helper can return 020 only for ModernEldrazi,
        // which UseArtAsFrame handles above; every remaining path returns 010.
        _ => "_typ_reg_sty_010",
    })
}

/// Apply the client's property-based frame overrides in precedence order.
fn frame_style(object: &ResolvedObject<'_>) -> Option<FrameStyle> {
    use FrameStyle::*;

    let mut frame = FrameStyle::from_raw(integer(object, "FRAMESTYLE", Invalid as i32)?);
    if object.field_ref("FUSE").is_some() {
        frame = FuseSplitCard;
    } else if object.field_ref("SPLITCARD0").is_some() {
        frame = SplitCard;
    }
    if frame == ModernFullArt && boolean(object, "IS_BASICLAND")? {
        frame = ZendikarBasicLand;
    }
    if boolean(object, "IS_EMBLEM")? {
        // Ring emblems and play aids retain their dedicated frame styles.
        if !matches!(frame, LTRRingEmblem | TOKPlayAid) {
            frame = Emblem;
        }
    } else if object.field_ref("EFFECT_NAME_STRING").is_some()
        && !boolean(object, "IS_SPELL")?
        && !boolean(object, "IS_PLAYER_AID")?
    {
        // Saga and class abilities use their full-art ability frames.
        frame = if boolean(object, "IS_SAGA")? {
            DominariaSagaAbility
        } else if boolean(object, "IS_CLASS")? || boolean(object, "IS_CASE")? {
            AFRClassAbility
        } else {
            AbilityOrEffect
        };
    }
    for (property, replacement) in [
        ("FACE_DOWN_BECAUSE_MORPH", Morph),
        ("MANIFEST_FACE_DOWN", Manifest),
        ("FACE_DOWN_BECAUSE_TEZZERET", TezzeretCreature),
        ("FACE_DOWN_BECAUSE_YEDORA", YedoraLand),
        ("FORETOLD_ORDINAL", Foretold),
        ("FACE_DOWN_BECAUSE_CLOAK", Cloak),
        ("FACE_DOWN_BECAUSE_DISGUISE", Cloak),
        ("FACE_DOWN", FaceDown),
    ] {
        // Face-down conditions override all preceding frame choices.
        if boolean(object, property)? {
            return Some(replacement);
        }
    }
    Some(if frame == Invalid { PostM15 } else { frame })
}
