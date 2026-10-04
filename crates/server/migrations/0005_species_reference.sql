-- Curated, hand-checked foraging-safety reference data. Owned by the
-- `species` module. See docs/ARCHITECTURE.md's "Database schema" section
-- (this file must match it verbatim) and the safety-critical note at the
-- top of that document: this data is never generated or overwritten by an
-- LLM call, only ever read and extended with model-identified *extras* that
-- are kept clearly separate from this curated set (see `deepdive` module).
--
-- Facts below are standard, widely-published foraging-safety facts repeated
-- across extension-service and field-guide sources (toxin names, "cook
-- thoroughly" caveats, classic lookalike pairs). Nothing here is invented.

CREATE TABLE species_reference (
    id            TEXT PRIMARY KEY,       -- slug, e.g. "amanita-phalloides"
    genus         TEXT NOT NULL,
    species       TEXT,                   -- NULL = genus-level entry
    common_names_json TEXT NOT NULL,       -- JSON array
    danger_level  TEXT NOT NULL,           -- DangerLevel
    wikipedia_title TEXT,                  -- best-known title to look up; NULL = derive from name
    summary       TEXT NOT NULL            -- 1-3 sentence curated safety-relevant summary
);

CREATE TABLE confusant_pairs (
    id              TEXT PRIMARY KEY,
    species_ref_a   TEXT NOT NULL REFERENCES species_reference(id),
    species_ref_b   TEXT NOT NULL REFERENCES species_reference(id),
    danger_level    TEXT NOT NULL,          -- danger of confusing A for B (usually B's danger)
    distinguishing_features_json TEXT NOT NULL, -- JSON array of strings, the checklist
    notes           TEXT NOT NULL
);
CREATE INDEX idx_confusant_a ON confusant_pairs(species_ref_a);
CREATE INDEX idx_confusant_b ON confusant_pairs(species_ref_b);

-- --------------------------------------------------------------------------
-- species_reference seed data
-- --------------------------------------------------------------------------

INSERT INTO species_reference (id, genus, species, common_names_json, danger_level, wikipedia_title, summary) VALUES
('agaricus-bisporus', 'Agaricus', 'bisporus',
 '["button mushroom","white mushroom","cremini","portobello"]', 'safe', 'Agaricus bisporus',
 'The common cultivated button mushroom (also sold as cremini or portobello at different maturities); safe and widely eaten when properly cooked, but its young pale caps are the most common point of confusion with deadly Amanita species.'),

('amanita-phalloides', 'Amanita', 'phalloides',
 '["death cap"]', 'deadly_toxic', 'Amanita phalloides',
 'One of the most lethally toxic mushrooms in the world and responsible for the majority of fatal mushroom poisonings; contains amatoxins that cause delayed, often irreversible liver failure.'),

('amanita-virosa', 'Amanita', 'virosa',
 '["destroying angel"]', 'deadly_toxic', 'Amanita virosa',
 'An all-white, deadly toxic Amanita containing the same amatoxins as the death cap; a single cap can be fatal.'),

('amanita-bisporigera', 'Amanita', 'bisporigera',
 '["eastern destroying angel"]', 'deadly_toxic', 'Amanita bisporigera',
 'The North American counterpart to Amanita virosa, equally deadly and amatoxin-containing.'),

('volvariella-volvacea', 'Volvariella', 'volvacea',
 '["straw mushroom","paddy straw mushroom"]', 'safe', 'Volvariella volvacea',
 'A widely cultivated edible species grown on rice straw, popular in Asian cuisine.'),

('morchella-esculenta', 'Morchella', 'esculenta',
 '["true morel","yellow morel"]', 'safe', 'Morchella esculenta',
 'The prized true/yellow morel, with a honeycomb-pitted cap; edible only when thoroughly cooked, never raw.'),

('gyromitra-esculenta', 'Gyromitra', 'esculenta',
 '["false morel"]', 'deadly_toxic', 'Gyromitra esculenta',
 'The false morel contains gyromitrin, a toxin that converts to a rocket-fuel-related compound in the body; it has caused fatalities even after traditional parboil-and-discard-water preparation and should not be eaten.'),

('cantharellus-cibarius', 'Cantharellus', 'cibarius',
 '["chanterelle","golden chanterelle"]', 'safe', 'Cantharellus cibarius',
 'A popular edible mushroom with blunt false gills (ridges) running down a funnel-shaped, apricot-colored cap.'),

('omphalotus-illudens', 'Omphalotus', 'illudens',
 '["jack-o''lantern mushroom"]', 'toxic', 'Omphalotus illudens',
 'A toxic, orange, cluster-forming species that causes severe gastrointestinal illness and is often mistaken for chanterelles.'),

('armillaria-mellea', 'Armillaria', 'mellea',
 '["honey fungus","honey mushroom"]', 'safe', 'Armillaria mellea',
 'An edible wood-decaying mushroom that grows in dense clusters; edible only when well-cooked, and can cause GI upset raw or undercooked.'),

('galerina-marginata', 'Galerina', 'marginata',
 '["deadly galerina","autumn skullcap"]', 'deadly_toxic', 'Galerina marginata',
 'A small, wood-growing mushroom containing the same amatoxins as the death cap, frequently mistaken for edible wood-growing clusters like honey fungus.'),

('cortinarius-rubellus', 'Cortinarius', 'rubellus',
 '["deadly webcap"]', 'deadly_toxic', 'Cortinarius rubellus',
 'Contains orellanine, a toxin causing delayed, severe and sometimes irreversible kidney failure days after ingestion.'),

('conium-maculatum', 'Conium', 'maculatum',
 '["poison hemlock"]', 'deadly_toxic', 'Conium maculatum',
 'One of the most toxic plants in North America and Europe, containing coniine, a neurotoxic alkaloid that causes fatal respiratory paralysis.'),

('daucus-carota', 'Daucus', 'carota',
 '["wild carrot","Queen Anne''s lace"]', 'safe', 'Daucus carota',
 'The edible ancestor of the cultivated carrot, but its carrot-family relatives include some of the deadliest plants, so positive identification is essential.'),

('cicuta-maculata', 'Cicuta', 'maculata',
 '["water hemlock"]', 'deadly_toxic', 'Cicuta maculata',
 'Considered one of the most acutely toxic plants in North America; its cicutoxin can cause fatal seizures within hours of ingestion.'),

('toxicoscordion-venenosum', 'Toxicoscordion', 'venenosum',
 '["death camas","meadow death camas"]', 'deadly_toxic', 'Toxicoscordion venenosum',
 'A deadly toxic bulb plant containing steroidal alkaloids, frequently mistaken for edible wild onions and camas lilies.'),

('allium-tricoccum', 'Allium', 'tricoccum',
 '["wild leek","ramps"]', 'safe', 'Allium tricoccum',
 'Ramps (wild leeks) are a popular edible wild-onion relative with a strong onion/garlic smell, a key feature distinguishing them from toxic lookalikes.'),

('convallaria-majalis', 'Convallaria', 'majalis',
 '["lily of the valley"]', 'toxic', 'Convallaria majalis',
 'An ornamental plant containing cardiac glycosides that are toxic if ingested, whose leaves are a known lookalike for edible wild garlic.'),

('allium-ursinum', 'Allium', 'ursinum',
 '["wild garlic","ramsons"]', 'safe', 'Allium ursinum',
 'Wild garlic (ramsons) is an edible wild allium with a strong garlic smell when crushed, which is the key safe way to distinguish it from toxic lookalikes like lily of the valley.');

-- --------------------------------------------------------------------------
-- confusant_pairs seed data
-- --------------------------------------------------------------------------

INSERT INTO confusant_pairs (id, species_ref_a, species_ref_b, danger_level, distinguishing_features_json, notes) VALUES
('confusant-death-cap-button',
 'amanita-phalloides', 'agaricus-bisporus', 'deadly_toxic',
 '["Dig up the whole base, never cut only the cap — death cap has a sac-like volva at the stem base that button mushrooms lack","Young death cap gills are white and stay pale; mature Agaricus gills turn pink then dark brown/black","Spore print: death cap = white; Agaricus = dark brown/blackish","A white volva at the base of ANY wild mushroom is a hard stop — do not eat it"]',
 'Death cap is the single most dangerous button-mushroom lookalike; most fatal mushroom poisonings worldwide are attributed to Amanita phalloides misidentified as a safe edible mushroom.'),

('confusant-destroying-angel-button',
 'amanita-virosa', 'agaricus-bisporus', 'deadly_toxic',
 '["Destroying angel is pure white all over, including the gills; mature button mushrooms have pink-to-brown gills","Check for a membranous ring on the stem AND a sac-like volva at the base — cultivated button mushrooms have neither","Never eat an all-white wild mushroom found growing from soil with a volva at the base"]',
 'Destroying angel is pure white and easily mistaken for a button mushroom at a young stage; always check for a volva and ring before considering it safe.'),

('confusant-false-morel-true-morel',
 'gyromitra-esculenta', 'morchella-esculenta', 'deadly_toxic',
 '["Slice it lengthwise top to bottom first: true morels are hollow all the way through; false morels are not hollow — they are cottony or chambered inside","True morel caps have a regular pitted honeycomb pattern; false morel caps look brain-like, wrinkled and lobed","Do not rely on cooking/parboiling to detoxify a suspected false morel — it has caused fatalities even after traditional preparation"]',
 'False morels have caused deaths even after traditional preparation methods; when in doubt, discard rather than cook.'),

('confusant-jack-o-lantern-chanterelle',
 'omphalotus-illudens', 'cantharellus-cibarius', 'toxic',
 '["Chanterelles have blunt, forking false ridges on the underside, not true gills; jack-o''lanterns have true, sharp-edged, closely-spaced gills","Chanterelles grow singly/scattered directly from soil; jack-o''lanterns grow in dense clusters from buried wood, roots, or stumps — dig to the base and check the substrate","Jack-o''lantern flesh may faintly bioluminesce (glow) in total darkness; chanterelles never do"]',
 'Jack-o''lantern mushrooms cause severe vomiting and diarrhea; checking the substrate and gill structure prevents this common mistake.'),

('confusant-deadly-galerina-honey-fungus',
 'galerina-marginata', 'armillaria-mellea', 'deadly_toxic',
 '["Both grow in clusters on wood — always take a spore print before eating any wood-growing cluster mushroom: Galerina = rusty brown, Armillaria = white","Armillaria mellea is generally more robust with a scaly cap and often white cottony mycelial mats under the bark; Galerina is slimmer and smooth-capped","Galerina''s amatoxins cause delayed symptoms (6-24h), by which time liver damage has already begun — never skip the spore-print check"]',
 'Both are wood-growing cluster mushrooms; a spore print is mandatory before eating any wild wood-growing mushroom cluster.'),

('confusant-deadly-webcap-chanterelle',
 'cortinarius-rubellus', 'cantharellus-cibarius', 'deadly_toxic',
 '["Young Cortinarius has a cobweb-like partial veil (cortina) on the stem — chanterelles never have this structure","Cortinarius has true gills; chanterelles have blunt false ridges, not true gills","Deadly webcap symptoms can be delayed by days (kidney failure) — never eat an unidentified gilled mushroom hoping it is a chanterelle"]',
 'Deadly webcap poisoning can be mistaken for a mild illness until kidney failure is already underway days later.'),

('confusant-poison-hemlock-wild-carrot',
 'conium-maculatum', 'daucus-carota', 'deadly_toxic',
 '["Poison hemlock stems are smooth and hairless, often with purple/red blotching near the base; wild carrot stems are hairy and uniformly green","Crush a leaf and smell it: wild carrot smells like carrot; poison hemlock has a musty, unpleasant ''mousy'' smell","Wild carrot''s root smells like carrot when cut; poison hemlock''s root does not","Poison hemlock is typically a much larger plant (up to 2-3m) than wild carrot (usually under 1m)"]',
 'Poison hemlock is a classic deadly lookalike for wild carrot and other carrot-family plants; never taste-test to identify.'),

('confusant-water-hemlock-wild-carrot',
 'cicuta-maculata', 'daucus-carota', 'deadly_toxic',
 '["Water hemlock grows in wet ground — ditches, stream banks, marshes; wild carrot prefers drier fields and roadsides","Cut the root/stem crown: water hemlock has chambered, hollow root crowns often with purple streaking; wild carrot has a simple pale taproot smelling of carrot","A few bites of water hemlock root can be fatal within hours — do not taste-test any carrot-family plant to identify it"]',
 'Water hemlock is among the most acutely toxic plants in North America; habitat and root-crown structure are the clearest tells.'),

('confusant-death-camas-wild-leek',
 'toxicoscordion-venenosum', 'allium-tricoccum', 'deadly_toxic',
 '["Smell test first, always: ramps/wild leeks smell strongly of onion/garlic when crushed; death camas has NO onion smell at all","Death camas bulbs lack the layered onion-like structure of true wild onions/leeks","Never harvest bulbs or grass-like leaves in wet meadows/slopes on the strength of appearance alone — the smell test is mandatory"]',
 'The onion/garlic smell test is the single most reliable and fastest way to rule out death camas before harvesting.'),

('confusant-lily-of-the-valley-wild-garlic',
 'convallaria-majalis', 'allium-ursinum', 'toxic',
 '["Crush a leaf: wild garlic (ramsons) smells strongly of garlic; lily-of-the-valley has no garlic smell at all","Lily-of-the-valley leaves have a stiffer, waxier texture with a rigid central fold and often grow in pairs from a shared base; wild garlic leaves are flatter, softer, and arise individually","Don''t rely on flowers to tell them apart — leaves of both often emerge before flowering"]',
 'Leaves of both plants look similar before flowering; the garlic smell test is the key distinguishing check.');
