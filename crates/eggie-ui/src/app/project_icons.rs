use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProjectIconCard {
    pub(super) id: Option<ItemId>,
    pub(super) icon: ProcessIcon,
}

#[derive(Clone, Copy)]
pub(super) enum ProjectIconLayout {
    Sidebar,
    SelectorTrigger,
    SelectorOption,
}

#[derive(Clone, Debug, PartialEq)]
struct PositionedCard {
    card: ProjectIconCard,
    pose: process_icons::CardPose,
}

#[derive(Clone, Debug, PartialEq)]
struct PaintOrder {
    cards: Vec<ProjectIconCard>,
    weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ProjectIconFrame {
    cards: Vec<PositionedCard>,
    orders: Vec<PaintOrder>,
    pub(super) width: f32,
}

impl ProjectIconFrame {
    fn settled(cards: &[ProjectIconCard]) -> Self {
        Self {
            cards: cards
                .iter()
                .enumerate()
                .map(|(index, &card)| PositionedCard {
                    card,
                    pose: process_icons::CardPose {
                        icon: card.icon,
                        offset_x: index as f32 * 5.,
                        angle: process_icons::stack_angle(index, cards.len()),
                        opacity: 1.,
                    },
                })
                .collect(),
            orders: vec![PaintOrder {
                cards: cards.iter().rev().copied().collect(),
                weight: 1.,
            }],
            width: process_icons::stack_width(cards.len(), PROJECT_CARD_SIZE),
        }
    }

    pub(super) fn layers(&self) -> Vec<process_icons::CardLayer> {
        self.orders
            .iter()
            .map(|order| process_icons::CardLayer {
                poses: order
                    .cards
                    .iter()
                    .filter_map(|key| self.cards.iter().find(|card| card.card == *key))
                    .map(|card| card.pose)
                    .collect(),
                weight: order.weight,
            })
            .collect()
    }

    fn toward(&self, target: &[ProjectIconCard], progress: f32) -> Self {
        if progress <= 0. {
            return self.clone();
        }
        let target = Self::settled(target);
        if progress >= 1. {
            return target;
        }
        let mut cards = self.cards.clone();
        for next in &target.cards {
            if cards.iter().any(|card| card.card == next.card) {
                continue;
            }
            let mut pose = next.pose;
            if next.card.icon != ProcessIcon::ProjectPlaceholder {
                if let Some(previous) = self
                    .cards
                    .iter()
                    .find(|card| card.card.id == next.card.id)
                    .or_else(|| {
                        self.cards
                            .iter()
                            .max_by(|a, b| a.pose.offset_x.total_cmp(&b.pose.offset_x))
                    })
                {
                    pose.offset_x = previous.pose.offset_x;
                    pose.angle = previous.pose.angle;
                }
            }
            pose.opacity = 0.;
            cards.push(PositionedCard {
                card: next.card,
                pose,
            });
        }
        for card in &mut cards {
            let to = target
                .cards
                .iter()
                .find(|next| next.card == card.card)
                .map(|next| next.pose)
                .unwrap_or_else(|| {
                    let replacement = target
                        .cards
                        .iter()
                        .find(|next| next.card.id == card.card.id);
                    let pose = replacement.map(|next| next.pose).unwrap_or(card.pose);
                    process_icons::CardPose {
                        icon: card.card.icon,
                        offset_x: pose.offset_x
                            + if replacement.is_some()
                                || card.card.icon == ProcessIcon::ProjectPlaceholder
                                || target
                                    .cards
                                    .iter()
                                    .any(|next| next.card.icon == ProcessIcon::ProjectPlaceholder)
                            {
                                0.
                            } else {
                                5.
                            },
                        opacity: 0.,
                        ..pose
                    }
                });
            card.pose.offset_x += (to.offset_x - card.pose.offset_x) * progress;
            card.pose.angle += (to.angle - card.pose.angle) * progress;
            card.pose.opacity += (to.opacity - card.pose.opacity) * progress;
        }
        let mut orders = Vec::<PaintOrder>::new();
        // Blend only the overlap between paint orders; card positions keep moving throughout.
        for (source, weight) in self
            .orders
            .iter()
            .map(|order| (order, order.weight * (1. - progress)))
            .chain(
                target
                    .orders
                    .iter()
                    .map(|order| (order, order.weight * progress)),
            )
        {
            let mut order = cards
                .iter()
                .filter(|card| !source.cards.contains(&card.card))
                .map(|card| card.card)
                .collect::<Vec<_>>();
            order.extend(source.cards.iter().copied());
            if let Some(existing) = orders.iter_mut().find(|existing| existing.cards == order) {
                existing.weight += weight;
            } else {
                orders.push(PaintOrder {
                    cards: order,
                    weight,
                });
            }
        }
        Self {
            cards,
            orders,
            width: self.width + (target.width - self.width) * progress,
        }
    }
}

#[derive(Clone)]
pub(super) struct ProjectIconState {
    pub(super) icons: Vec<ProjectIconCard>,
    from: ProjectIconFrame,
    pub(super) revision: u64,
    pub(super) started_at: Instant,
}

impl ProjectIconState {
    pub(super) fn new(icons: Vec<ProjectIconCard>, now: Instant) -> Self {
        Self {
            from: ProjectIconFrame::settled(&icons),
            icons,
            revision: 0,
            started_at: now - PROJECT_ICON_TRANSITION,
        }
    }

    pub(super) fn retarget(&mut self, icons: Vec<ProjectIconCard>, now: Instant) {
        if self.icons == icons {
            return;
        }
        // Continue from the displayed positions, overlap and label width when interrupted.
        self.from = self.frame(now);
        self.icons = icons;
        self.revision += 1;
        self.started_at = now;
    }

    pub(super) fn frame(&self, now: Instant) -> ProjectIconFrame {
        let progress = (now.saturating_duration_since(self.started_at).as_secs_f32()
            / PROJECT_ICON_TRANSITION.as_secs_f32())
        .clamp(0., 1.);
        self.from.toward(&self.icons, gpui::ease_in_out(progress))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn card(id: u128, icon: ProcessIcon) -> ProjectIconCard {
        ProjectIconCard {
            id: Some(Uuid::from_u128(id)),
            icon,
        }
    }

    fn pixels(frame: &ProjectIconFrame) -> Vec<u8> {
        process_icons::card_layers_frame(
            &frame.layers(),
            false,
            PROJECT_CARD_SIZE,
            process_icons::stack_width(PROJECT_MAX_ICON_COUNT, PROJECT_CARD_SIZE),
        )
        .as_bytes(0)
        .unwrap()
        .to_vec()
    }

    fn assert_close_pixels(a: &ProjectIconFrame, b: &ProjectIconFrame) {
        let a = pixels(a);
        let b = pixels(b);
        assert_eq!(a.len(), b.len());
        let delta = a.iter().zip(b).map(|(a, b)| a.abs_diff(b)).max().unwrap();
        assert!(delta <= 2, "paint-order discontinuity: pixel delta {delta}");
    }

    #[test]
    fn deleting_front_card_preserves_its_initial_occlusion() {
        let cards = [
            card(1, ProcessIcon::ClaudeCode),
            card(2, ProcessIcon::BunJs),
            card(3, ProcessIcon::Python),
        ];
        let from = ProjectIconFrame::settled(&cards);
        let target = &cards[1..];
        assert_eq!(pixels(&from), pixels(&from.toward(target, 0.)));
        assert_close_pixels(&from, &from.toward(target, 0.00001));
        let middle = from.toward(target, 0.5);
        let departing = middle.cards.iter().find(|c| c.card == cards[0]).unwrap();
        assert_eq!(departing.pose.offset_x, 2.5);
        assert_eq!(departing.pose.opacity, 0.5);
        let promoted = middle.cards.iter().find(|c| c.card == cards[1]).unwrap();
        assert_eq!(promoted.pose.offset_x, 2.5);
        assert_eq!(promoted.pose.opacity, 1.);
        let settled = ProjectIconFrame::settled(target);
        assert_close_pixels(&settled, &from.toward(target, 0.99999));
        assert_eq!(pixels(&settled), pixels(&from.toward(target, 1.)));
    }

    #[test]
    fn swapping_front_and_back_cards_preserves_both_endpoints() {
        let cards = [
            card(1, ProcessIcon::ClaudeCode),
            card(2, ProcessIcon::BunJs),
            card(3, ProcessIcon::Python),
        ];
        let target = [cards[2], cards[1], cards[0]];
        let from = ProjectIconFrame::settled(&cards);
        let settled = ProjectIconFrame::settled(&target);
        assert_close_pixels(&from, &from.toward(&target, 0.00001));
        assert_close_pixels(&settled, &from.toward(&target, 0.99999));
        let middle = from.toward(&target, 0.5);
        assert_eq!(middle.orders.len(), 2);
        assert!(middle.cards.iter().all(|c| c.pose.opacity == 1.));
        assert!(middle.cards.iter().all(|c| c.pose.offset_x == 5.));
        assert_eq!(pixels(&settled), pixels(&from.toward(&target, 1.)));
    }

    #[test]
    fn retargeting_preserves_the_current_pose_layers_and_label_width() {
        let now = Instant::now();
        let first = card(1, ProcessIcon::Terminal);
        let second = card(2, ProcessIcon::BunJs);
        let third = card(3, ProcessIcon::Python);
        let mut state = ProjectIconState::new(vec![first], now);
        state.retarget(vec![first, second, third], now);
        let halfway = now + PROJECT_ICON_TRANSITION / 2;
        let displayed = state.frame(halfway);
        state.retarget(vec![third, first], halfway);
        assert_eq!(state.frame(halfway), displayed);
        assert_eq!(pixels(&state.frame(halfway)), pixels(&displayed));
        let next = state.frame(halfway + Duration::from_micros(1));
        assert_close_pixels(&displayed, &next);
        assert!((next.width - displayed.width).abs() < 0.001);
        let end = state.frame(halfway + PROJECT_ICON_TRANSITION);
        assert_eq!(end, ProjectIconFrame::settled(&[third, first]));
    }

    #[test]
    fn adding_and_removing_back_cards_moves_and_rotates_them() {
        let first = card(1, ProcessIcon::Terminal);
        let second = card(2, ProcessIcon::BunJs);
        let from = ProjectIconFrame::settled(&[first]);
        let adding = from.toward(&[first, second], 0.5);
        let entering = adding.cards.iter().find(|c| c.card == second).unwrap();
        assert_eq!(entering.pose.offset_x, 2.5);
        assert_eq!(entering.pose.angle, 3.5);
        assert_eq!(entering.pose.opacity, 0.5);
        assert_eq!(adding.cards[0].pose.angle, -3.5);
        let removing = ProjectIconFrame::settled(&[first, second]).toward(&[first], 0.5);
        assert_eq!(removing.cards[1].pose.offset_x, 7.5);
        assert_eq!(removing.cards[1].pose.opacity, 0.5);
        assert_eq!(removing.cards[0].pose.angle, -3.5);
    }

    #[test]
    fn placeholder_transition_stays_upright_and_stationary() {
        let placeholder = ProjectIconCard {
            id: None,
            icon: ProcessIcon::ProjectPlaceholder,
        };
        let terminal = card(1, ProcessIcon::Terminal);
        for frame in [
            ProjectIconFrame::settled(&[placeholder]).toward(&[terminal], 0.5),
            ProjectIconFrame::settled(&[terminal]).toward(&[placeholder], 0.5),
        ] {
            assert!(frame.cards.iter().all(|c| c.pose.angle == 0.));
            assert!(frame.cards.iter().all(|c| c.pose.offset_x == 0.));
            let empty = frame.cards.iter().find(|c| c.card == placeholder).unwrap();
            assert_eq!(empty.pose.offset_x, 0.);
            assert_eq!(empty.pose.opacity, 0.5);
        }
    }

    #[test]
    fn foreground_process_change_keeps_the_same_cards_position() {
        let old = card(1, ProcessIcon::NodeJs);
        let new = card(1, ProcessIcon::ClaudeCode);
        let back = card(2, ProcessIcon::Python);
        let frame = ProjectIconFrame::settled(&[old, back]).toward(&[new, back], 0.5);
        let changing = frame
            .cards
            .iter()
            .filter(|c| c.card.id == old.id)
            .collect::<Vec<_>>();
        assert_eq!(changing.len(), 2);
        assert!(
            changing
                .iter()
                .all(|c| c.pose.offset_x == 0. && c.pose.angle == -7.)
        );
        assert!(changing.iter().all(|c| c.pose.opacity == 0.5));
    }
}
